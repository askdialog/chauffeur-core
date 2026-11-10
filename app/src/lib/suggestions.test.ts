import { describe, expect, test } from "bun:test";
import { type ComposerState, dismissedSuggestions, shownSuggestion, suggestionKey } from "./suggestions";

const offered = { text: "Run the tests", key: "e3-suggestion" };
const empty: ComposerState = { text: "", images: 0, working: false, dismissed: null };
const press = (key: string, fields: Partial<Parameters<typeof suggestionKey>[1]> = {}) => ({
  key,
  shiftKey: false,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  isComposing: false,
  ...fields,
});

describe("the suggested next message in the composer", () => {
  test("shows in an empty composer of an idle chat", () => {
    expect(shownSuggestion(offered, empty)).toEqual(offered);
    expect(shownSuggestion(null, empty)).toBeNull();
    expect(shownSuggestion({ text: "  ", key: "k" }, empty)).toBeNull();
  });

  test("hides while the user writes, attaches an image, or the agent works", () => {
    expect(shownSuggestion(offered, { ...empty, text: "R" })).toBeNull();
    expect(shownSuggestion(offered, { ...empty, text: " " })).toBeNull();
    expect(shownSuggestion(offered, { ...empty, images: 1 })).toBeNull();
    expect(shownSuggestion(offered, { ...empty, working: true })).toBeNull();
  });

  test("comes back when what the user typed is erased", () => {
    const typing = { ...empty, text: "R" };
    expect(shownSuggestion(offered, typing)).toBeNull();
    expect(shownSuggestion(offered, { ...typing, text: "" })).toEqual(offered);
  });

  test("set aside stays aside; the next suggestion shows again", () => {
    expect(shownSuggestion(offered, { ...empty, dismissed: offered.key })).toBeNull();
    expect(shownSuggestion({ text: "Commit", key: "e9-suggestion" }, { ...empty, dismissed: offered.key })).not.toBeNull();
  });

  test("Tab takes it, Esc sets it aside; other keys and chords type as usual", () => {
    expect(suggestionKey(offered, press("Tab"))).toBe("accept");
    expect(suggestionKey(offered, press("Escape"))).toBe("dismiss");
    expect(suggestionKey(offered, press("Tab", { shiftKey: true }))).toBeNull();
    expect(suggestionKey(offered, press("Tab", { metaKey: true }))).toBeNull();
    expect(suggestionKey(offered, press("Tab", { isComposing: true }))).toBeNull();
    expect(suggestionKey(offered, press("Enter"))).toBeNull();
    expect(suggestionKey(offered, press("a"))).toBeNull();
  });

  test("without a suggestion shown, Tab and Esc are left to the composer (slash menu, interrupt)", () => {
    expect(suggestionKey(null, press("Tab"))).toBeNull();
    expect(suggestionKey(null, press("Escape"))).toBeNull();
  });

  test("a suggestion set aside is remembered per chat", () => {
    expect(dismissedSuggestions.get("chat-a")).toBeNull();
    dismissedSuggestions.set("chat-a", offered.key);
    expect(dismissedSuggestions.get("chat-a")).toBe(offered.key);
    expect(dismissedSuggestions.get("chat-b")).toBeNull();
  });
});
