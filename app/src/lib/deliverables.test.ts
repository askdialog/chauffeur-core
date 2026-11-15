import { expect, test } from "bun:test";
import { deliverableDownloadUrl, parseDeliverable, versionPath } from "./deliverables";

const path = ".chauffeur/artifacts/data-loop/index.html";
test("a document keeps its source identity and only distinct exports", () => {
  expect(parseDeliverable(JSON.stringify({ title: " Data Loop ", path, exports: [path, ".chauffeur/artifacts/data-loop/report.pdf"] })))
    .toEqual({ title: "Data Loop", path, exports: [".chauffeur/artifacts/data-loop/report.pdf"], kind: "document" });
});

test("a deck is named by the deck.json of its own folder", () => {
  const deck = ".chauffeur/artifacts/q3/deck.json";
  expect(parseDeliverable(JSON.stringify({ title: "Q3", path: deck, kind: "deck" }))).toEqual({ title: "Q3", path: deck, exports: [], kind: "deck" });
  for (const invalid of [".chauffeur/artifacts/deck.json", ".chauffeur/artifacts/q3/index.html", ".chauffeur/artifacts/../deck.json"]) {
    expect(parseDeliverable(JSON.stringify({ title: "Q3", path: invalid, kind: "deck" }))).toBeNull();
  }
  expect(parseDeliverable(JSON.stringify({ title: "Q3", path: deck }))).toBeNull();
  expect(parseDeliverable(JSON.stringify({ title: "Q3", path: deck, kind: "slides" }))).toBeNull();
});

test("incomplete, external and traversing document declarations remain ordinary code", () => {
  for (const value of ["{", "null", "[]", JSON.stringify({ path }), JSON.stringify({ title: "", path })]) expect(parseDeliverable(value)).toBeNull();
  for (const invalid of ["/tmp/report.html", "https://example.com/a.html", ".chauffeur/artifacts/../secret.html", ".chauffeur/artifacts/%2e%2e/a.html", ".chauffeur/artifacts/a.html?x", ".chauffeur/artifacts/a\\b.html", ".chauffeur/artifacts/a.pptx", ".chauffeur/artifacts//a.html"]) {
    expect(parseDeliverable(JSON.stringify({ title: "Report", path: invalid }))).toBeNull();
    expect(parseDeliverable(JSON.stringify({ title: "Report", path, exports: [invalid] }))).toBeNull();
  }
});

test("a browser download URL contains only its scoped ticket", () => {
  expect(deliverableDownloadUrl("http://localhost:123/deliverables/w?token=private&path=old#old", "once"))
    .toBe("http://localhost:123/deliverables/w?ticket=once");
});

test("an earlier version lies in its deliverable folder's .versions", () => {
  expect(versionPath(".chauffeur/artifacts/report/index.html", 3)).toBe(".chauffeur/artifacts/report/.versions/3/index.html");
  expect(versionPath(".chauffeur/artifacts/report/pdf/report.pdf", 1)).toBe(".chauffeur/artifacts/report/.versions/1/pdf/report.pdf");
  expect(versionPath(".chauffeur/artifacts/alone.html", 1)).toBeNull();
});
