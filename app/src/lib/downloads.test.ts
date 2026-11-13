import { describe, expect, test } from "bun:test";
import { savedMessage } from "./downloads";

describe("savedMessage", () => {
  test("names the file or folder the shell wrote", () => {
    expect(savedMessage(1, "/Users/a/Downloads/cat.png")).toBe("Saved to /Users/a/Downloads/cat.png");
    expect(savedMessage(3, "/Users/a/Downloads/Chauffeur attachments")).toBe("Saved 3 files to /Users/a/Downloads/Chauffeur attachments");
  });
});
