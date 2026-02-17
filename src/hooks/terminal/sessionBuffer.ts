export type SessionBuffer = {
  chunks: string[];
  start: number;
  totalChars: number;
  cachedText: string | null;
};

export const MAX_SESSION_BUFFER_CHUNKS = 2_048;

function visibleChunkCount(buffer: SessionBuffer) {
  return buffer.chunks.length - buffer.start;
}

function compactBufferIfNeeded(buffer: SessionBuffer) {
  if (buffer.start < 256) return;
  if (buffer.start * 2 < buffer.chunks.length) return;
  buffer.chunks = buffer.chunks.slice(buffer.start);
  buffer.start = 0;
}

function ensureBuffer(store: Map<string, SessionBuffer>, sessionId: string): SessionBuffer {
  let buffer = store.get(sessionId);
  if (!buffer) {
    buffer = {
      chunks: [],
      start: 0,
      totalChars: 0,
      cachedText: null,
    };
    store.set(sessionId, buffer);
  }
  return buffer;
}

export function appendSessionBuffer(
  store: Map<string, SessionBuffer>,
  sessionId: string,
  data: string,
  maxChars: number,
  maxChunks = MAX_SESSION_BUFFER_CHUNKS
) {
  if (!data) return;
  const buffer = ensureBuffer(store, sessionId);

  buffer.chunks.push(data);
  buffer.totalChars += data.length;
  buffer.cachedText = null;

  while (buffer.totalChars > maxChars && buffer.start < buffer.chunks.length) {
    const removed = buffer.chunks[buffer.start] ?? "";
    buffer.totalChars -= removed.length;
    buffer.start += 1;
  }

  while (visibleChunkCount(buffer) > maxChunks && buffer.start < buffer.chunks.length) {
    const removed = buffer.chunks[buffer.start] ?? "";
    buffer.totalChars -= removed.length;
    buffer.start += 1;
  }

  if (buffer.totalChars < 0) {
    buffer.totalChars = 0;
  }

  compactBufferIfNeeded(buffer);
}

export function readSessionBuffer(store: Map<string, SessionBuffer>, sessionId: string): string {
  const buffer = store.get(sessionId);
  if (!buffer) return "";
  if (buffer.cachedText !== null) return buffer.cachedText;
  const text = buffer.chunks.slice(buffer.start).join("");
  buffer.cachedText = text;
  return text;
}
