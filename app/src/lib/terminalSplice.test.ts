import { describe, expect, test } from "bun:test";
import { OutputSplicer } from "./terminalSplice";

const bytes = (text: string) => new TextEncoder().encode(text);
const text = (chunks: Uint8Array[]) => chunks.map((c) => new TextDecoder().decode(c)).join("");

function setup() {
  const out: Uint8Array[] = [];
  let gaps = 0;
  const splicer = new OutputSplicer(
    (b) => out.push(b),
    () => {
      gaps += 1;
    },
  );
  return { splicer, out, gaps: () => gaps };
}

describe("OutputSplicer", () => {
  test("chunks held before the snapshot are skipped, trimmed or written", () => {
    const { splicer, out } = setup();
    // Whole output: "$ ls\r\nalpha beta\r\n$ " — the snapshot has the first 10 bytes.
    splicer.push(0, bytes("$ ls\r\n")); // entirely in the snapshot
    splicer.push(6, bytes("alpha beta")); // straddles end_offset 10
    out.push(bytes("$ ls\r\nalph")); // the snapshot, written by the caller
    splicer.snapshotWritten(10);
    splicer.push(16, bytes("\r\n$ "));
    expect(text(out)).toBe("$ ls\r\nalpha beta\r\n$ ");
    expect(splicer.position).toBe(20);
  });

  test("duplicates after the snapshot are dropped", () => {
    const { splicer, out } = setup();
    splicer.snapshotWritten(5);
    splicer.push(0, bytes("hello"));
    splicer.push(5, bytes(" world"));
    splicer.push(5, bytes(" world"));
    expect(text(out)).toBe(" world");
  });

  test("a gap asks for a new snapshot and holds the chunk for it", () => {
    const { splicer, out, gaps } = setup();
    splicer.snapshotWritten(3);
    splicer.push(3, bytes("abc"));
    splicer.push(10, bytes("XYZ")); // bytes 6..10 were missed
    expect(gaps()).toBe(1);
    expect(splicer.position).toBeNull();
    splicer.push(13, bytes("!"));
    // The new snapshot covers up to 11: "XYZ" is trimmed to "YZ", then "!".
    splicer.snapshotWritten(11);
    expect(text(out)).toBe("abcYZ!");
  });

  test("an empty terminal starts at offset 0", () => {
    const { splicer, out } = setup();
    splicer.snapshotWritten(0);
    splicer.push(0, bytes("first"));
    expect(text(out)).toBe("first");
  });
});
