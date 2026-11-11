import { expect, test } from "bun:test";
import { deliverableDownloadUrl, parseDeliverable } from "./deliverables";

const path = ".chauffeur/artifacts/data-loop/index.html";
test("a document keeps its source identity and only distinct exports", () => {
  expect(parseDeliverable(JSON.stringify({ title: " Data Loop ", path, exports: [path, ".chauffeur/artifacts/data-loop/report.pdf"] })))
    .toEqual({ title: "Data Loop", path, exports: [".chauffeur/artifacts/data-loop/report.pdf"] });
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
