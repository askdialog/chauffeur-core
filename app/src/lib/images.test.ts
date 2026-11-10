import { describe, expect, test } from "bun:test";
import {
  checkFileSize,
  checkMessageTotal,
  fitWithin,
  formatBytes,
  imageFiles,
  isImageFile,
  MAX_EDGE,
  MAX_FILE_BYTES,
  MAX_MESSAGE_FILE_BYTES,
  readAttachment,
  toImageInputs,
} from "./images";

describe("attached images", () => {
  test("large images are scaled so their long side fits, small ones kept", () => {
    expect(fitWithin(3024, 1964)).toEqual({ width: MAX_EDGE, height: 1299 });
    expect(fitWithin(1000, 4000)).toEqual({ width: 500, height: MAX_EDGE });
    expect(fitWithin(800, 600)).toEqual({ width: 800, height: 600 });
  });

  test("only the image types the agents read are kept", () => {
    expect(isImageFile({ type: "image/png" })).toBe(true);
    expect(isImageFile({ type: "image/heic" })).toBe(false);
    const files = [new File(["a"], "a.png", { type: "image/png" }), new File(["b"], "b.pdf", { type: "application/pdf" })];
    expect(imageFiles(files).map((f) => f.name)).toEqual(["a.png"]);
    expect(imageFiles(null)).toEqual([]);
  });

  test("pending images become the daemon's ImageInput", () => {
    expect(toImageInputs([])).toBeUndefined();
    expect(toImageInputs([{ id: "1", name: "shot.png", mediaType: "image/png", data: "iVBO", previewUrl: "blob:x", size: 4 }])).toEqual([
      { name: "shot.png", media_type: "image/png", data: "iVBO" },
    ]);
    // A file the browser has no type for leaves it to the daemon.
    expect(toImageInputs([{ id: "2", name: "build.log", mediaType: "", data: "aGk=", previewUrl: null, size: 2 }])).toEqual([
      { name: "build.log", data: "aGk=" },
    ]);
  });

  test("any other file goes as its bytes, without a thumbnail, up to the daemon's limit", async () => {
    const page = await readAttachment(new File(["<html></html>"], "report.html", { type: "text/html" }));
    expect(page).toMatchObject({ name: "report.html", mediaType: "text/html", previewUrl: null, size: 13 });
    expect(atob(page.data)).toBe("<html></html>");
    const huge = { name: "big.zip", type: "application/zip", size: MAX_FILE_BYTES + 1 } as File;
    await expect(readAttachment(huge)).rejects.toThrow("over the 20 MB limit");
  });

  test("a file is refused over the daemon's limit before it is read, an image never", () => {
    expect(() => checkFileSize({ type: "", size: 21 << 20 })).toThrow("21 MB is over the 20 MB limit");
    expect(() => checkFileSize({ type: "", size: MAX_FILE_BYTES })).not.toThrow();
    expect(() => checkFileSize({ type: "image/png", size: 3 * MAX_FILE_BYTES })).not.toThrow();
  });

  test("sizes read as people say them", () => {
    expect(formatBytes(90)).toBe("90 B");
    expect(formatBytes(812 * 1024)).toBe("812 KB");
    expect(formatBytes(2.4 * 1024 * 1024)).toBe("2.4 MB");
    expect(formatBytes(20 << 20)).toBe("20 MB");
  });

  test("a message's files stay under the daemon's message limit once base64-encoded", () => {
    // Base64 takes 4 bytes for 3; the daemon accepts 64 MB in one message.
    expect((MAX_MESSAGE_FILE_BYTES * 4) / 3).toBeLessThan((64 << 20) - (4 << 20));
    expect(() => checkMessageTotal(MAX_FILE_BYTES, MAX_FILE_BYTES)).not.toThrow();
    expect(() => checkMessageTotal(2 * MAX_FILE_BYTES, 1)).toThrow("limited to 40 MB together");
  });
});
