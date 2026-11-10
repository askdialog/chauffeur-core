// Splits a stream of chunks into lines in time linear in its length, however it is cut: a line of
// megabytes in thousands of chunks (an attachment, a screenshot in base64) is joined once, not
// searched again from its start at each chunk.

export class LineSplitter {
  private readonly decoder = new TextDecoder();
  private partial: string[] = [];

  /** Calls `onLine` with each line `chunk` completes, without its newline. */
  push(chunk: Uint8Array | string, onLine: (line: string) => void): void {
    const text = typeof chunk === "string" ? chunk : this.decoder.decode(chunk, { stream: true });
    let start = 0;
    let newline = text.indexOf("\n");
    while (newline >= 0) {
      const head = text.slice(start, newline);
      let line = head;
      if (this.partial.length > 0) {
        this.partial.push(head);
        line = this.partial.join("");
        this.partial = [];
      }
      onLine(line);
      start = newline + 1;
      newline = text.indexOf("\n", start);
    }
    if (start < text.length) this.partial.push(text.slice(start));
  }

  /** What follows the last newline, once the input ended. */
  end(): string {
    this.partial.push(this.decoder.decode());
    const rest = this.partial.join("");
    this.partial = [];
    return rest;
  }
}
