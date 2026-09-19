//! Bounded MCP observation branch for PTY output (MCP V1 phase A, ORQ-28).
//!
//! The human terminal byte stream is untouched: the PTY reader still feeds
//! the existing emitter path unchanged. In parallel each decoded chunk is
//! pushed into a per-connection ring buffer with a hard byte budget and a
//! monotonically increasing sequence number.
//!
//! A slow or absent MCP client may drop old history, but reads that fall
//! behind always report the gap explicitly (`gap_from_seq`) instead of
//! silently skipping bytes, and the buffer never grows without bound, so it
//! can never stall or bloat the human terminal path.
//!
//! `grant_start_seq` (phase B) is simply a sequence number captured from the
//! backend at authorization time; the buffer itself does not know about
//! authorization, it only serves data from an explicit start position.

use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Mutex;

/// Per-connection retention cap. Roughly 1 MiB of UTF-16 characters.
pub const MCP_BUFFER_MAX_CHARS_PER_CONNECTION: usize = 1_000_000;
/// Process-wide retention cap across all connections.
pub const MCP_BUFFER_GLOBAL_MAX_CHARS: usize = 8 * 1_000_000;
/// Single read responses never exceed this. ~32 KiB in UTF-16 units.
pub const MCP_READ_MAX_CHARS: usize = 32 * 1024;

#[derive(Debug, Clone)]
struct BufferChunk {
    seq: u64,
    data: String,
}

struct ConnectionBuffer {
    chunks: VecDeque<BufferChunk>,
    total_chars: usize,
    dropped_chars: usize,
}

impl ConnectionBuffer {
    fn new() -> Self {
        Self {
            chunks: VecDeque::new(),
            total_chars: 0,
            dropped_chars: 0,
        }
    }

    fn push(&mut self, seq: u64, data: String) -> Vec<String> {
        self.total_chars += data.len();
        self.chunks.push_back(BufferChunk { seq, data });
        self.evict_overflow()
    }

    fn evict_overflow(&mut self) -> Vec<String> {
        let mut evicted = Vec::new();
        while self.total_chars > MCP_BUFFER_MAX_CHARS_PER_CONNECTION {
            let Some(chunk) = self.chunks.pop_front() else {
                break;
            };
            self.total_chars -= chunk.data.len();
            self.dropped_chars += chunk.data.len();
            evicted.push(chunk.data);
        }
        evicted
    }

    fn first_seq(&self) -> Option<u64> {
        self.chunks.front().map(|chunk| chunk.seq)
    }

    fn last_seq(&self) -> Option<u64> {
        self.chunks.back().map(|chunk| chunk.seq)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputReadResult {
    /// Chunks with seq >= the effective start cursor, concatenation order
    /// preserved. Never exceeds `MCP_READ_MAX_CHARS` total characters.
    pub chunks: Vec<BufferReadChunk>,
    /// Cursor to pass as `from_seq` on the next read.
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
    pub seq: u64,
    pub data: String,
}

pub struct OutputBuffers {
    inner: Mutex<HashMap<String, ConnectionBuffer>>,
    next_seq: AtomicU64,
}

impl Default for OutputBuffers {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            next_seq: AtomicU64::new(1),
        }
    }
}

impl OutputBuffers {
    fn allocate_seq(&self) -> u64 {
        self.next_seq.fetch_add(1, AtomicOrdering::SeqCst)
    }

    /// Current append position for a connection without mutating anything.
    /// Phase B captures this at authorization time so agents only receive
    /// output produced after the grant.
    pub fn current_seq(&self) -> u64 {
        self.next_seq.load(AtomicOrdering::SeqCst)
    }

    /// Feed one decoded chunk. Returns the assigned sequence number, or None
    /// if the buffer map is poisoned. Cheap and allocation-bounded; called
    /// from the PTY reader thread so it must never block on consumers.
    pub fn push(&self, connection_id: &str, data: String) -> Option<u64> {
        if data.is_empty() {
            return None;
        }
        let seq = self.allocate_seq();
        let mut inner = self.inner.lock().ok()?;
        let buffer = inner
            .entry(connection_id.to_string())
            .or_insert_with(ConnectionBuffer::new);
        let evicted = buffer.push(seq, data);

        if !evicted.is_empty() {
            // Enforce the global cap by evicting oldest chunks from the
            // largest buffer(s). `evicted` itself is dropped here.
            let mut global: usize = inner.values().map(|b| b.total_chars).sum();
            while global > MCP_BUFFER_GLOBAL_MAX_CHARS {
                let victim_key = inner
                    .iter()
                    .max_by_key(|(_, b)| b.total_chars)
                    .map(|(k, _)| k.clone());
                let Some(key) = victim_key else { break };
                let Some(buffer) = inner.get_mut(&key) else { break };
                let Some(chunk) = buffer.chunks.pop_front() else { break };
                buffer.total_chars -= chunk.data.len();
                buffer.dropped_chars += chunk.data.len();
                global -= chunk.data.len();
            }
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
            next_seq: buffer.last_seq().map(|s| s + 1).unwrap_or(from_seq),
            gap_from_seq: None,
            truncated: false,
        };

        let Some(first_seq) = buffer.first_seq() else {
            return Some(result);
        };
        if from_seq < first_seq {
            result.gap_from_seq = Some(from_seq);
        }

        let mut budget = max_chars;
        for chunk in buffer.chunks.iter() {
            if chunk.seq < from_seq {
                continue;
            }
            if budget == 0 {
                result.truncated = true;
                result.next_seq = chunk.seq;
                break;
            }
            if chunk.data.len() > budget {
                let prefix = safe_prefix(&chunk.data, budget);
                result.chunks.push(BufferReadChunk {
                    seq: chunk.seq,
                    data: prefix,
                });
                result.truncated = true;
                result.next_seq = chunk.seq;
                break;
            }
            budget -= chunk.data.len();
            result.chunks.push(BufferReadChunk {
                seq: chunk.seq,
                data: chunk.data.clone(),
            });
        }
        Some(result)
    }

    #[allow(dead_code)] // retained as phase B/C integration surface
    pub fn remove(&self, connection_id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.remove(connection_id);
        }
    }

    /// Number of buffered characters for a connection (diagnostics/tests).
    #[allow(dead_code)] // retained as phase B/C integration surface
    pub fn buffered_chars(&self, connection_id: &str) -> usize {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.get(connection_id).map(|b| b.total_chars))
            .unwrap_or(0)
    }
}

/// Take at most `max_chars` chars without splitting a surrogate pair.
fn safe_prefix(data: &str, max_chars: usize) -> String {
    let mut out: String = data.chars().take(max_chars).collect();
    if out.len() == data.len() {
        return out;
    }
    // The char-boundary take above already guarantees UTF-8 validity; this
    // trims a dangling UTF-16 high surrogate so consumers doing UTF-16
    // accounting (the JS side) never see a torn pair.
    if out
        .chars()
        .last()
        .map(|c| {
            let code = c as u32;
            (0xD800..=0xDBFF).contains(&code)
        })
        .unwrap_or(false)
    {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(read.next_seq, s2 + 1);
    }

    #[test]
    fn evicted_history_reports_explicit_gap() {
        let buffers = OutputBuffers::default();
        let first = buffers.push("c1", "x".repeat(1024)).unwrap();
        for _ in 0..(MCP_BUFFER_MAX_CHARS_PER_CONNECTION / 1024) {
            buffers.push("c1", "y".repeat(1024)).unwrap();
        }
        let read = buffers.read("c1", first, MCP_READ_MAX_CHARS).unwrap();
        assert_eq!(read.gap_from_seq, Some(first));
        assert!(read.chunks.iter().all(|c| c.data.chars().all(|ch| ch == 'y')));
        assert!(buffers.buffered_chars("c1") <= MCP_BUFFER_MAX_CHARS_PER_CONNECTION);
    }

    #[test]
    fn per_read_budget_truncates_without_losing_order() {
        let buffers = OutputBuffers::default();
        let start = buffers.push("c1", "a".repeat(100)).unwrap();
        buffers.push("c1", "b".repeat(100)).unwrap();

        let read = buffers.read("c1", start, 120).unwrap();
        let total: usize = read.chunks.iter().map(|c| c.data.chars().count()).sum();
        assert_eq!(total, 120);
        assert!(read.truncated);
        // Resume from the reported cursor and continue in order.
        let rest = buffers.read("c1", read.next_seq, MCP_READ_MAX_CHARS).unwrap();
        let tail: String = rest.chunks.iter().map(|c| c.data.as_str()).collect();
        assert_eq!(tail.chars().take(1).collect::<String>(), "b");
    }

    #[test]
    fn safe_prefix_never_tears_utf8_or_surrogate_tail() {
        let s = "héllo 中文";
        let prefix = safe_prefix(s, 6);
        assert_eq!(prefix, "héllo ");
        assert!(std::str::from_utf8(prefix.as_bytes()).is_ok());
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
}
