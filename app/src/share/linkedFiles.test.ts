import { expect, test } from "bun:test";
import type { DesignComment } from "@chauffeur/protocol";
import { carriedFiles, commentsOn, displayName, linkedTarget, normalizeRef } from "./linkedFiles";

test("a name is read as the daemon reads it", () => {
  expect(normalizeRef("`./docs/a.md:42`")).toBe("docs/a.md");
  expect(normalizeRef("docs/a.md#L3-L9")).toBe("docs/a.md");
  expect(normalizeRef("a.md:4:2")).toBe("a.md");
  expect(normalizeRef("a.md,")).toBe("a.md");
  expect(normalizeRef("https://x.com/a.md")).toBeNull();
  expect(normalizeRef("two words.md")).toBeNull();
});

test("a name opens the file the manifest gives for the file it is written in", () => {
  const manifest = { "RECAP.md": { "MODELISATION.md": "_chauffeur/s/MODELISATION.md", "detail/AUDIT.md": "_chauffeur/s/detail/AUDIT.md" } };
  expect(linkedTarget(manifest, "RECAP.md", "MODELISATION.md")).toBe("_chauffeur/s/MODELISATION.md");
  expect(linkedTarget(manifest, "RECAP.md", "detail%2FAUDIT.md")).toBe("_chauffeur/s/detail/AUDIT.md");
  expect(linkedTarget(manifest, "_chauffeur/s/MODELISATION.md", "MODELISATION.md")).toBeNull();
  expect(linkedTarget(manifest, "RECAP.md", "OTHER.md")).toBeNull();
  expect(carriedFiles("RECAP.md", manifest)).toEqual(["RECAP.md", "_chauffeur/s/MODELISATION.md", "_chauffeur/s/detail/AUDIT.md"]);
  expect(displayName("_chauffeur/s/MODELISATION.md")).toBe("MODELISATION.md");
});

test("a file's comments are its threads and their replies", () => {
  const author = { name: "Bob", email: "bob@example.com" };
  const comment = (id: string, screen: string, reply_to?: string): DesignComment => ({ id, screen, author, at: 0, text: id, reply_to, mentions: [] });
  const all = [comment("a", "RECAP.md"), comment("b", "_chauffeur/s/M.md"), comment("c", "", "b"), comment("d", "", "a")];
  expect(commentsOn(all, "_chauffeur/s/M.md").map((c) => c.id)).toEqual(["b", "c"]);
  expect(commentsOn(all, "RECAP.md").map((c) => c.id)).toEqual(["a", "d"]);
});
