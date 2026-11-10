import { describe, expect, test } from "bun:test";
import type { DetectedPreview, DetectedService } from "@chauffeur/protocol";
import {
  allowedChoices,
  badPreviewNames,
  blockedChoices,
  initialChoices,
  initialPreviews,
  savedChoices,
  savedPreviews,
} from "./devServices";

const service = (change: Partial<DetectedService> = {}): DetectedService => ({
  name: "db",
  container: "aaaaaaaaaaaa",
  image: "postgres:16",
  running: true,
  compose: { project: "shop", file: "shop-api/docker-compose.yml", service: "db" },
  postgres: { username: "shop", databases: ["shop"] },
  data: true,
  problem: null,
  ...change,
});

describe("development services", () => {
  test("only a running PostgreSQL server with databases and a compose file can carry data", () => {
    expect(allowedChoices(service())).toEqual(["data", "cache", "skip"]);
    expect(allowedChoices(service({ running: false }))).toEqual(["cache", "skip"]);
    expect(allowedChoices(service({ postgres: null, data: false, image: "redis:7" }))).toEqual(["cache", "skip"]);
    // Started without a compose file: only rebuilt elsewhere.
    expect(allowedChoices(service({ compose: null, problem: "not started from a compose file" }))).toEqual([
      "cache",
      "skip",
    ]);
    // Docker restarts it on its own: it cannot be declared at all.
    expect(allowedChoices(service({ problem: "restart: always" }))).toEqual(["skip"]);
  });

  test("the dialog starts from what is declared, else from the suggestion", () => {
    const detected = [service(), service({ name: "cache", postgres: null, data: false, image: "redis:7" })];
    expect(initialChoices(detected, null)).toEqual({ db: "data", cache: "cache" });
    const declared = {
      revision: 3,
      services: [{ name: "db", image: "postgres:16", compose: null, data: false, databases: [] }],
      previews: [],
    };
    expect(initialChoices(detected, declared)).toEqual({ db: "cache", cache: "cache" });
    expect(initialChoices([service({ problem: "restart: always" })], null)).toEqual({ db: "skip" });
  });

  test("a server declared as data stays so while it cannot be read, and saving waits", () => {
    const stopped = [service({ running: false, postgres: { username: "shop", databases: [] } })];
    const declared = {
      revision: 1,
      services: [{ name: "db", image: "postgres:16", compose: null, data: true, databases: ["shop"] }],
      previews: [],
    };
    const choices = initialChoices(stopped, declared);
    expect(choices).toEqual({ db: "data" });
    expect(blockedChoices(stopped, choices)).toEqual(["db"]);
    expect(blockedChoices(stopped, { db: "cache" })).toEqual([]);
  });

  test("a declared service whose container is gone waits for an explicit choice", () => {
    const gone = [service({ running: false, postgres: null, data: false, problem: "its container is not found" })];
    const declared = {
      revision: 2,
      services: [{ name: "db", image: "redis:7", compose: null, data: false, databases: [] }],
      previews: [],
    };
    const choices = initialChoices(gone, declared);
    expect(choices).toEqual({ db: "cache" });
    expect(blockedChoices(gone, choices)).toEqual(["db"]);
    expect(blockedChoices(gone, { db: "skip" })).toEqual([]);
  });

  test("services left out are not saved", () => {
    expect(savedChoices({ db: "data", cache: "cache", web: "skip" })).toEqual([
      { name: "db", data: true },
      { name: "cache", data: false },
    ]);
  });
});

describe("web previews", () => {
  const preview = (change: Partial<DetectedPreview> = {}): DetectedPreview => ({
    name: "dev",
    port: 41000,
    offset: 0,
    listening: true,
    declared: false,
    ...change,
  });

  test("a port that answers is offered unchecked, a declared one stays checked even when silent", () => {
    const drafts = initialPreviews([
      preview(),
      preview({ name: "api", port: 41001, offset: 1, listening: false, declared: true }),
    ]);
    expect(drafts).toEqual({ 41000: { name: "dev", declare: false }, 41001: { name: "api", declare: true } });
    expect(savedPreviews(drafts)).toEqual([{ name: "api", port: 41001 }]);
  });

  test("the checked ports are saved under their names, which must be valid and distinct", () => {
    const drafts = {
      41000: { name: " web ", declare: true },
      41001: { name: "web", declare: true },
      41002: { name: "docs site", declare: true },
      41003: { name: "", declare: false },
    };
    expect(savedPreviews(drafts)).toEqual([
      { name: "web", port: 41000 },
      { name: "web", port: 41001 },
      { name: "docs site", port: 41002 },
    ]);
    expect(badPreviewNames(drafts)).toEqual([41000, 41001, 41002]);
    expect(badPreviewNames({ 41000: { name: "web", declare: true }, 41001: { name: "", declare: false } })).toEqual([]);
  });
});
