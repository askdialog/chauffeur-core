import { describe, expect, test } from "bun:test";
import { Drafts } from "./drafts";

function memory() {
  const data = new Map<string, string>();
  return { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => void data.set(k, v) };
}

describe("composer drafts", () => {
  test("a chat's text is kept, and survives a restart once written", () => {
    const storage = memory();
    const drafts = new Drafts(storage);
    drafts.setText("chat-a", "half a thought");
    drafts.setText("chat-b", "other");
    expect(drafts.text("chat-a")).toBe("half a thought");
    drafts.flush();
    expect(new Drafts(storage).text("chat-a")).toBe("half a thought");
  });

  test("an emptied composer leaves no draft", () => {
    const storage = memory();
    const drafts = new Drafts(storage);
    drafts.setText("chat-a", "sent soon");
    drafts.setText("chat-a", "");
    drafts.flush();
    expect(storage.getItem("chauffeur.drafts")).toBe("{}");
  });

  test("a composer shown is told when its draft is written from elsewhere", () => {
    const drafts = new Drafts(null);
    const seen: string[] = [];
    const stop = drafts.subscribe("chat-a", (text) => seen.push(text));
    drafts.setText("chat-a", "from the canvas");
    drafts.setText("chat-a", "from the canvas");
    drafts.setText("chat-b", "another chat");
    stop();
    drafts.setText("chat-a", "after");
    expect(seen).toEqual(["from the canvas"]);
  });

  test("attachments are kept while the app runs", () => {
    const drafts = new Drafts(null);
    const file = { id: "1", name: "logs.zip", mediaType: "application/zip", data: "UEs=", previewUrl: null, size: 2 };
    drafts.setFiles("chat-a", [file]);
    expect(drafts.files("chat-a")).toEqual([file]);
    drafts.setFiles("chat-a", []);
    expect(drafts.files("chat-a")).toEqual([]);
  });

  test("a broken store starts empty", () => {
    const storage = memory();
    storage.setItem("chauffeur.drafts", "not json");
    expect(new Drafts(storage).text("x")).toBe("");
  });
});
