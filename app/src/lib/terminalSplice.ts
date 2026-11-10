// Joins a terminal's scrollback snapshot and its live output without losing or repeating bytes.
//
// `terminal.snapshot` returns the scrollback and `end_offset`, the offset of the byte after it in
// the terminal's whole output; each `terminal_output` chunk carries the offset of its first byte.
// Live chunks that arrive before the snapshot are held, then replayed after it: a chunk entirely
// before `end_offset` is skipped, one that straddles it is trimmed. A chunk that starts after the
// last byte written means output was missed (e.g. while the connection was down): the caller
// takes a fresh snapshot.

export class OutputSplicer {
  /** Offset of the byte after the last one written; null while waiting for a snapshot. */
  private written: number | null = null;
  private held: Array<{ offset: number; bytes: Uint8Array }> = [];

  constructor(
    private readonly write: (bytes: Uint8Array) => void,
    private readonly onGap: () => void,
  ) {}

  get position(): number | null {
    return this.written;
  }

  /** A live chunk. */
  push(offset: number, bytes: Uint8Array) {
    if (this.written == null) {
      this.held.push({ offset, bytes });
      return;
    }
    this.apply(offset, bytes);
  }

  /** The snapshot's bytes were written; `endOffset` is the snapshot's `end_offset`. */
  snapshotWritten(endOffset: number) {
    this.written = endOffset;
    const held = this.held;
    this.held = [];
    for (const chunk of held) {
      if (this.written == null) {
        // A gap while replaying: hold the rest for the next snapshot.
        this.held.push(chunk);
        continue;
      }
      this.apply(chunk.offset, chunk.bytes);
    }
  }

  /** Waits for a new snapshot (live chunks are held meanwhile). */
  reset() {
    this.written = null;
  }

  private apply(offset: number, bytes: Uint8Array) {
    const written = this.written!;
    const end = offset + bytes.length;
    if (end <= written) return;
    if (offset > written) {
      this.written = null;
      this.held.push({ offset, bytes });
      this.onGap();
      return;
    }
    this.write(offset < written ? bytes.subarray(written - offset) : bytes);
    this.written = end;
  }
}
