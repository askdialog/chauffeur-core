import { expect, test } from "bun:test";
import type { ExtensionInfo, ExtensionPackageInfo } from "@chauffeur/protocol";
import { availableLayout, extensionEnabled, packageStatus, viewAvailable } from "./extensions";
import { DEFAULT_LAYOUT } from "./splitLayout";

const installed: ExtensionPackageInfo = {
  desired_installed: true,
  installed: true,
  auto_update: true,
  available: null,
  phase: "installed",
  error: null,
  ui: { revision: "r", base_url: "http://127.0.0.1:1/extension-assets/x/r/n/", script: "ui/extension.js", stylesheet: null, assets: "ui/assets" },
  can_restore: false,
};

const extension = (id: "database" | "office", enabled: boolean, pkg: Partial<ExtensionPackageInfo> | null = {}): ExtensionInfo => ({
  id,
  name: id === "database" ? "Database" : "Office",
  description: "",
  publisher: "Chauffeur",
  version: "1.0.0",
  enabled,
  capabilities: [],
  package: pkg === null ? null : { ...installed, ...pkg },
});

const extensions: ExtensionInfo[] = [extension("database", false), extension("office", true)];

test("Core views remain available before the extension catalogue arrives", () => {
  expect(viewAvailable(null, "chat")).toBe(true);
  expect(viewAvailable(null, "terminal")).toBe(true);
  expect(viewAvailable(null, "database")).toBe(false);
});

test("only enabled extension contributions can be opened", () => {
  expect(extensionEnabled(extensions, "office")).toBe(true);
  expect(viewAvailable(extensions, "office")).toBe(true);
  expect(viewAvailable(extensions, "database")).toBe(false);
});

test("a removed extension's views cannot be opened, even enabled", () => {
  expect(viewAvailable([extension("database", true, { desired_installed: false, installed: false, ui: null })], "database")).toBe(false);
  expect(viewAvailable([extension("database", true, null)], "database")).toBe(false);
});

test("an extension starting, downloading, missing its files or failed keeps its views", () => {
  for (const phase of ["starting", "downloading", "missing", "failed", "waiting"] as const) {
    expect(viewAvailable([extension("database", true, { phase, installed: phase !== "missing", ui: null })], "database")).toBe(true);
  }
});

test("disabling an extension removes its saved side pane and reopen target", () => {
  const layout = { ...DEFAULT_LAYOUT, side: "database", last: "database" } as const;
  const next = availableLayout(extensions, layout);
  expect(next.side).toBeNull();
  expect(next.last).toBe("terminal");
  expect(next.size).toEqual(layout.size);
  expect(availableLayout(extensions.map((entry) => ({ ...entry, enabled: true })), layout)).toBe(layout);
});

test("a saved layout survives until the catalogue arrives and through an update", () => {
  const layout = { ...DEFAULT_LAYOUT, side: "database", last: "database" } as const;
  expect(availableLayout(null, layout)).toBe(layout);
  expect(availableLayout([extension("database", true, { phase: "starting", ui: null })], layout)).toBe(layout);
});

test("the Marketplace says what each package is doing", () => {
  expect(packageStatus(extension("database", true))).toEqual({ label: "Installed", tone: "success", detail: null, busy: false });
  expect(packageStatus(extension("database", true, { available: "1.1.0" }))).toMatchObject({ label: "Update available", detail: "Version 1.1.0 is available." });
  expect(packageStatus(extension("database", true, { phase: "downloading", available: "1.1.0" }))).toMatchObject({ label: "Downloading", busy: true });
  expect(packageStatus(extension("database", true, { phase: "waiting", available: "1.1.0" })).detail).toContain("applies once this extension's views are closed");
  expect(packageStatus(extension("database", true, { phase: "starting" }))).toMatchObject({ busy: true });
  // A failed check or download beside a running engine is an error to read, not a failure.
  expect(packageStatus(extension("database", true, { phase: "waiting", available: "1.2.0", error: "the download failed" }))).toMatchObject({ label: "Update ready", tone: "attention", detail: "the download failed" });
  expect(packageStatus(extension("database", true, { error: "the download failed" }))).toMatchObject({ label: "Installed", tone: "attention" });
  expect(packageStatus(extension("database", true, { phase: "failed", error: "engine exited" }))).toMatchObject({ label: "Failed", tone: "danger", detail: "engine exited" });
  expect(packageStatus(extension("database", true, { error: "1.2.0 needs another Chauffeur version" }))).toMatchObject({ tone: "attention", detail: "1.2.0 needs another Chauffeur version" });
  expect(packageStatus(extension("database", true, { desired_installed: false, installed: false }))).toMatchObject({ label: "Not installed", busy: false });
  expect(packageStatus(extension("database", true, { desired_installed: false, phase: "downloading" }))).toMatchObject({ label: "Removing", busy: true });
});
