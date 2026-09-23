//! Bounded MCP observation branch for PTY output (MCP V1 phase A, ORQ-28).
//!
//! Each decoded PTY chunk is copied into a per-connection ring buffer with a
//! hard byte budget and a monotonically increasing global byte position. The
//! MCP copy can evict history without changing the terminal event payload.
//!
//! A slow or absent MCP client may lose old history, but reads that fall
//! behind always report the gap explicitly (`gap_from_seq`) instead of
//! silently skipping bytes, and retained MCP history never grows beyond its
//! per-connection or process-wide byte cap.
//!
//! `grant_start_seq` (phase B) is simply a sequence number captured from the
//! backend at authorization time; the buffer itself does not know about
//! authorization, it only serves data from an explicit start position.

use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Mutex;

/// Per-connection retention cap measured in UTF-8 bytes.
pub const MCP_BUFFER_MAX_BYTES_PER_CONNECTION: usize = 1_000_000;
/// Process-wide retention cap measured in UTF-8 bytes across all connections.
pub const MCP_BUFFER_GLOBAL_MAX_BYTES: usize = 8 * 1_000_000;
/// Compatibility alias. The retained-size limit is measured in UTF-8 bytes.
#[allow(dead_code)]
pub const MCP_BUFFER_MAX_CHARS_PER_CONNECTION: usize = MCP_BUFFER_MAX_BYTES_PER_CONNECTION;
/// Compatibility alias. The retained-size limit is measured in UTF-8 bytes.
#[allow(dead_code)]
pub const MCP_BUFFER_GLOBAL_MAX_CHARS: usize = MCP_BUFFER_GLOBAL_MAX_BYTES;
/// Single read responses never exceed this number of UTF-8 bytes.
pub const MCP_READ_MAX_BYTES: usize = 32 * 1024;
/// Maximum caller-requested character count, retained for the MCP maxChars contract.
pub const MCP_READ_MAX_CHARS: usize = 32 * 1024;

#[derive(Debug, Clone)]
struct BufferChunk {
    seq: u64,
    data: String,
}

struct ConnectionBuffer {
    chunks: VecDeque<BufferChunk>,
    total_bytes: usize,
    dropped_bytes: usize,
    capture_gap_end_seq: u64,
}

impl ConnectionBuffer {
    fn new() -> Self {
        Self {
            chunks: VecDeque::new(),
            total_bytes: 0,
            dropped_bytes: 0,
            capture_gap_end_seq: 0,
        }
    }

    fn push(&mut self, seq: u64, data: String) {
        self.total_bytes += data.len();
        self.chunks.push_back(BufferChunk { seq, data });
        self.evict_overflow();
    }

    fn evict_overflow(&mut self) {
        while self.total_bytes > MCP_BUFFER_MAX_BYTES_PER_CONNECTION {
            let Some(chunk) = self.chunks.pop_front() else {
                break;
            };
            self.total_bytes -= chunk.data.len();
            self.dropped_bytes += chunk.data.len();
        }
    }

    fn first_seq(&self) -> Option<u64> {
        self.chunks.front().map(|chunk| chunk.seq)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputReadResult {
    /// Chunks with seq >= the effective start cursor, concatenation order
    /// preserved. Never exceeds `MCP_READ_MAX_BYTES` UTF-8 bytes or the
    /// caller's max character count.
    pub chunks: Vec<BufferReadChunk>,
    /// Global UTF-8 byte position to pass as `from_seq` on the next read.
    pub next_seq: u64,
    /// Set when the requested cursor points at data that has been evicted.
    /// The caller must treat the skipped range as lost; we never substitute
    /// data from another connection and never silently resume mid-stream.
    pub gap_from_seq: Option<u64>,
    /// True when more buffered data exists beyond this response.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferReadChunk {
    /// Global UTF-8 byte position at which this returned substring begins.
    pub seq: u64,
    pub data: String,
}

pub struct OutputBuffers {
    inner: Mutex<HashMap<String, ConnectionBuffer>>,
    next_seq: AtomicU64,
    unattributed_gap_end_seq: AtomicU64,
}

impl Default for OutputBuffers {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            next_seq: AtomicU64::new(1),
            unattributed_gap_end_seq: AtomicU64::new(0),
        }
    }
}

impl OutputBuffers {
    fn allocate_seq(&self, byte_count: usize) -> u64 {
        self.next_seq
            .fetch_add(byte_count as u64, AtomicOrdering::SeqCst)
    }

    /// Current global append position in UTF-8 bytes, without mutating anything.
    /// Phase B captures this at authorization time so agents only receive
    /// output produced after the grant.
    pub fn current_seq(&self) -> u64 {
        self.next_seq.load(AtomicOrdering::SeqCst)
    }

    /// Record output skipped because a lifecycle gate was contended. This path
    /// never blocks the PTY reader on the MCP buffer lock.
    pub fn record_capture_gap(&self, connection_id: &str, byte_count: usize) {
        if byte_count == 0 {
            return;
        }
        let end_seq = self
            .allocate_seq(byte_count)
            .saturating_add(byte_count as u64);
        match self.inner.try_lock() {
            Ok(mut inner) => {
                let buffer = inner
                    .entry(connection_id.to_string())
                    .or_insert_with(ConnectionBuffer::new);
                buffer.capture_gap_end_seq = buffer.capture_gap_end_seq.max(end_seq);
            }
            Err(_) => {
                self.unattributed_gap_end_seq
                    .fetch_max(end_seq, AtomicOrdering::SeqCst);
            }
        }
    }

    /// Feed one decoded chunk. Returns its starting global byte position, or
    /// None if the buffer map is poisoned. Called from the PTY reader thread.
    pub fn push(&self, connection_id: &str, data: String) -> Option<u64> {
        if data.is_empty() {
            return None;
        }
        let mut inner = self.inner.lock().ok()?;
        let seq = self.allocate_seq(data.len());
        let buffer = inner
            .entry(connection_id.to_string())
            .or_insert_with(ConnectionBuffer::new);
        buffer.push(seq, data);

        // Check the process-wide cap after every append, including appends
        // which did not trigger per-connection eviction.
        let mut global_bytes: usize = inner.values().map(|b| b.total_bytes).sum();
        while global_bytes > MCP_BUFFER_GLOBAL_MAX_BYTES {
            // Evict the globally oldest retained chunk first.
            let victim_key = inner
                .iter()
                .filter_map(|(key, buffer)| buffer.chunks.front().map(|chunk| (key, chunk.seq)))
                .min_by_key(|(_, seq)| *seq)
                .map(|(key, _)| key.clone());
            let Some(key) = victim_key else { break };
            let Some(buffer) = inner.get_mut(&key) else {
                break;
            };
            let Some(chunk) = buffer.chunks.pop_front() else {
                break;
            };
            let bytes = chunk.data.len();
            buffer.total_bytes -= bytes;
            buffer.dropped_bytes += bytes;
            global_bytes -= bytes;
        }
        Some(seq)
    }

    /// Read buffered chunks for one connection starting at `from_seq`.
    /// Returns an explicit gap when the cursor is older than what is still
    /// retained; an empty (but valid) result when the cursor is at the tail.
    pub fn read(
        &self,
        connection_id: &str,
        from_seq: u64,
        max_chars: usize,
    ) -> Option<OutputReadResult> {
        let max_chars = max_chars.min(MCP_READ_MAX_CHARS);
        let inner = self.inner.lock().ok()?;
        let buffer = inner.get(connection_id)?;
        let mut result = OutputReadResult {
            chunks: Vec::new(),
            next_seq: from_seq,
            gap_from_seq: None,
            truncated: false,
        };

        let gap_end = buffer
            .capture_gap_end_seq
            .max(self.unattributed_gap_end_seq.load(AtomicOrdering::SeqCst));
        if from_seq < gap_end {
            result.gap_from_seq = Some(from_seq);
        }

        let Some(first_seq) = buffer.first_seq() else {
            return Some(result);
        };
        if from_seq < first_seq && result.gap_from_seq.is_none() {
            result.gap_from_seq = Some(from_seq);
            result.next_seq = first_seq;
        }

        let mut char_budget = max_chars;
        let mut byte_budget = MCP_READ_MAX_BYTES;
        for chunk in buffer.chunks.iter() {
            let chunk_end = chunk.seq + chunk.data.len() as u64;
            if chunk_end <= result.next_seq {
                continue;
            }

            let byte_offset = result
                .next_seq
                .saturating_sub(chunk.seq)
                .min(chunk.data.len() as u64) as usize;
            let byte_offset = safe_byte_offset_at_or_after(&chunk.data, byte_offset);
            let chunk_cursor = chunk.seq + byte_offset as u64;

            if char_budget == 0 || byte_budget == 0 {
                result.truncated = true;
                result.next_seq = chunk_cursor;
                break;
            }

            let remaining = &chunk.data[byte_offset..];
            if remaining.is_empty() {
                result.next_seq = chunk_cursor;
                continue;
            }
            let char_limited = safe_prefix_chars(remaining, char_budget);
            let prefix = safe_prefix_bytes(&char_limited, byte_budget);
            let prefix_chars = prefix.chars().count();
            let remaining_chars = remaining.chars().count();
            let consumed_bytes = prefix.len();
            if consumed_bytes > 0 {
                result.chunks.push(BufferReadChunk {
                    seq: chunk_cursor,
                    data: prefix,
                });
                result.next_seq = chunk_cursor + consumed_bytes as u64;
            }
            if prefix_chars < remaining_chars {
                result.truncated = true;
                break;
            }
            char_budget -= prefix_chars;
            byte_budget -= consumed_bytes;
            result.next_seq = chunk_end;
        }
        Some(result)
    }

    #[allow(dead_code)] // retained as phase B/C integration surface
    pub fn remove(&self, connection_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.remove(connection_id);
        }
    }

    /// Drop every retained MCP observation buffer at a security lifecycle
    /// boundary. This also removes buffers for connections no longer present
    /// in the registry, so callers do not need a best-effort connection list.
    pub fn clear_all(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.clear();
        }
        self.unattributed_gap_end_seq
            .store(0, AtomicOrdering::SeqCst);
    }

    /// Number of buffered UTF-8 bytes for a connection (diagnostics/tests).
    #[allow(dead_code)] // retained as phase B/C integration surface
    pub fn buffered_bytes(&self, connection_id: &str) -> usize {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.get(connection_id).map(|b| b.total_bytes))
            .unwrap_or(0)
    }

    /// Compatibility alias; the returned retained size is measured in bytes.
    #[allow(dead_code)] // retained as phase B/C integration surface
    pub fn buffered_chars(&self, connection_id: &str) -> usize {
        self.buffered_bytes(connection_id)
    }

    /// Total retained UTF-8 bytes across all connections (test diagnostics).
    #[cfg(test)]
    pub fn global_buffered_bytes(&self) -> usize {
        self.inner
            .lock()
            .ok()
            .map(|inner| inner.values().map(|buffer| buffer.total_bytes).sum())
            .unwrap_or(usize::MAX)
    }
}

/// Take at most `max_chars` Unicode scalar values.
fn safe_prefix_chars(data: &str, max_chars: usize) -> String {
    data.chars().take(max_chars).collect()
}

/// Take at most `max_bytes` bytes without splitting a UTF-8 code point.
fn safe_prefix_bytes(data: &str, max_bytes: usize) -> String {
    let end = data
        .char_indices()
        .map(|(index, ch)| index + ch.len_utf8())
        .take_while(|end| *end <= max_bytes)
        .last()
        .unwrap_or(0);
    data[..end].to_string()
}

/// Round an arbitrary byte cursor forward to the next UTF-8 character boundary.
fn safe_byte_offset_at_or_after(data: &str, offset: usize) -> usize {
    let offset = offset.min(data.len());
    if offset == 0 || offset == data.len() || data.is_char_boundary(offset) {
        return offset;
    }
    data.char_indices()
        .find(|(index, _)| *index > offset)
        .map(|(index, _)| index)
        .unwrap_or(data.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn reads_in_order_with_monotonic_seqs() {
        let buffers = OutputBuffers::default();
        let s1 = buffers.push("c1", "hello ".to_string()).unwrap();
        let s2 = buffers.push("c1", "world".to_string()).unwrap();
        assert!(s2 > s1);

        let read = buffers.read("c1", s1, MCP_READ_MAX_CHARS).unwrap();
        assert!(read.gap_from_seq.is_none());
        assert!(!read.truncated);
        let joined: String = read.chunks.iter().map(|c| c.data.as_str()).collect();
        assert_eq!(joined, "hello world");
        assert_eq!(read.next_seq, s2 + "world".len() as u64);
    }

    #[test]
    fn evicted_history_reports_explicit_gap() {
        let buffers = OutputBuffers::default();
        let first = buffers.push("c1", "x".repeat(1024)).unwrap();
        for _ in 0..(MCP_BUFFER_MAX_BYTES_PER_CONNECTION / 1024) {
            buffers.push("c1", "y".repeat(1024)).unwrap();
        }
        let read = buffers.read("c1", first, MCP_READ_MAX_CHARS).unwrap();
        assert_eq!(read.gap_from_seq, Some(first));
        assert!(read
            .chunks
            .iter()
            .all(|c| c.data.chars().all(|ch| ch == 'y')));
        assert!(buffers.buffered_bytes("c1") <= MCP_BUFFER_MAX_BYTES_PER_CONNECTION);
    }

    #[test]
    fn per_read_budget_truncates_without_losing_order() {
        let buffers = OutputBuffers::default();
        let start = buffers.push("c1", "a".repeat(100)).unwrap();
        buffers.push("c1", "b".repeat(100)).unwrap();

        let read = buffers.read("c1", start, 120).unwrap();
        let total: usize = read.chunks.iter().map(|c| c.data.len()).sum();
        assert_eq!(total, 120);
        assert!(read.truncated);
        // Resume from the reported cursor and continue in order.
        let rest = buffers
            .read("c1", read.next_seq, MCP_READ_MAX_CHARS)
            .unwrap();
        let tail: String = rest.chunks.iter().map(|c| c.data.as_str()).collect();
        assert_eq!(tail.chars().take(1).collect::<String>(), "b");
    }

    #[test]
    fn global_retention_cap_applies_when_new_connection_does_not_evict() {
        let buffers = OutputBuffers::default();
        for index in 0..9 {
            buffers.push(
                &format!("c{index}"),
                "x".repeat(MCP_BUFFER_MAX_BYTES_PER_CONNECTION),
            );
        }

        let retained_bytes: usize = buffers
            .inner
            .lock()
            .unwrap()
            .values()
            .map(|buffer| buffer.total_bytes)
            .sum();
        assert!(retained_bytes <= MCP_BUFFER_GLOBAL_MAX_BYTES);
    }

    #[test]
    fn truncated_read_resumes_inside_a_chunk_at_exact_utf8_boundary() {
        let buffers = OutputBuffers::default();
        let start = buffers.push("c1", "a猫🙂z".to_string()).unwrap();

        let first = buffers.read("c1", start, 2).unwrap();
        let first_data: String = first
            .chunks
            .iter()
            .map(|chunk| chunk.data.as_str())
            .collect();
        assert_eq!(first_data, "a猫");
        assert_eq!(first.next_seq, start + "a猫".len() as u64);
        assert!(first.truncated);

        let second = buffers.read("c1", first.next_seq, 2).unwrap();
        let second_data: String = second
            .chunks
            .iter()
            .map(|chunk| chunk.data.as_str())
            .collect();
        assert_eq!(second_data, "🙂z");
        assert!(!second.truncated);
        assert_eq!(second.next_seq, start + "a猫🙂z".len() as u64);
    }

    #[test]
    fn skipped_pty_capture_advances_cursor_and_reports_gap() {
        let buffers = OutputBuffers::default();
        let start = buffers.current_seq();
        buffers.record_capture_gap("c1", "missed output".len());

        let read = buffers.read("c1", start, MCP_READ_MAX_CHARS).unwrap();
        assert!(read.gap_from_seq.is_some());
        assert!(read.chunks.is_empty());
        assert_eq!(buffers.current_seq(), start + "missed output".len() as u64);
    }

    #[test]
    fn read_never_exceeds_max_utf8_bytes_for_emoji_output() {
        let buffers = OutputBuffers::default();
        let start = buffers.push("c1", "🙂".repeat(MCP_READ_MAX_BYTES)).unwrap();

        let read = buffers.read("c1", start, usize::MAX).unwrap();
        let response_bytes: usize = read.chunks.iter().map(|chunk| chunk.data.len()).sum();
        assert_eq!(response_bytes, MCP_READ_MAX_BYTES);
        assert!(response_bytes <= MCP_READ_MAX_BYTES);
        assert!(read.truncated);
        assert_eq!(read.next_seq, start + MCP_READ_MAX_BYTES as u64);
    }

    #[test]
    fn safe_prefix_never_tears_utf8_or_surrogate_tail() {
        let s = "héllo 中文";
        let prefix = safe_prefix_chars(s, 6);
        assert_eq!(prefix, "héllo ");
        assert!(std::str::from_utf8(prefix.as_bytes()).is_ok());
    }

    #[test]
    fn byte_prefix_stops_before_partial_utf8_character() {
        let prefix = safe_prefix_bytes("猫🙂z", 5);
        assert_eq!(prefix, "猫");
        assert!(prefix.len() <= 5);
    }

    #[test]
    fn unknown_connection_reads_fail_closed() {
        let buffers = OutputBuffers::default();
        assert!(buffers.read("nope", 1, 1024).is_none());
    }

    #[test]
    fn remove_drops_connection_buffer() {
        let buffers = OutputBuffers::default();
        buffers.push("c1", "data".to_string());
        assert!(buffers.buffered_chars("c1") > 0);
        buffers.remove("c1");
        assert_eq!(buffers.buffered_chars("c1"), 0);
    }

    #[test]
    #[ignore = "explicit release-only MCP performance baseline"]
    fn ignored_continuous_output_retention_performance_baseline() {
        const CONNECTION_COUNT: usize = 8;
        const CHUNK_BYTES: usize = 64 * 1024;
        const CHUNKS_PER_CONNECTION: usize = 512;

        // Keep one reusable payload and let OutputBuffers enforce the bounded
        // retention policy. No reads occur during the push loop, modelling a
        // slow/absent MCP reader without allocating the full logical stream.
        let payload = "x".repeat(CHUNK_BYTES);
        let buffers = OutputBuffers::default();
        let started = Instant::now();
        for round in 0..CHUNKS_PER_CONNECTION {
            for connection_index in 0..CONNECTION_COUNT {
                let connection_id = format!("perf-{connection_index}");
                assert!(buffers.push(&connection_id, payload.clone()).is_some());
            }
            if round % 64 == 0 {
                assert!(buffers.global_buffered_bytes() <= MCP_BUFFER_GLOBAL_MAX_BYTES);
            }
        }
        let elapsed = started.elapsed();
        let logical_bytes = CONNECTION_COUNT * CHUNKS_PER_CONNECTION * CHUNK_BYTES;
        let retained_bytes = buffers.global_buffered_bytes();

        assert!(retained_bytes <= MCP_BUFFER_GLOBAL_MAX_BYTES);
        for connection_index in 0..CONNECTION_COUNT {
            let connection_id = format!("perf-{connection_index}");
            assert!(buffers.buffered_bytes(&connection_id) <= MCP_BUFFER_MAX_BYTES_PER_CONNECTION);

            // A reader that starts from an old cursor receives a bounded page
            // and an explicit gap, never a response larger than 32 KiB.
            let read = buffers
                .read(&connection_id, 0, usize::MAX)
                .expect("the connection retained output");
            let read_bytes: usize = read.chunks.iter().map(|chunk| chunk.data.len()).sum();
            assert!(read_bytes <= MCP_READ_MAX_BYTES);
        }

        let throughput_mib_per_second =
            logical_bytes as f64 / elapsed.as_secs_f64() / (1024.0 * 1024.0);
        println!(
            concat!(
                "mcp_perf_output logical_bytes={} retained_bytes={} ",
                "elapsed_ms={:.3} throughput_mib_s={:.3}"
            ),
            logical_bytes,
            retained_bytes,
            elapsed.as_secs_f64() * 1000.0,
            throughput_mib_per_second
        );
    }
}
