import { expect, test } from "bun:test";
import type { OllamaPull } from "@chauffeur/protocol";
import { installLabel, pullLabel, pullPercent, sizeLabel } from "./localModels";

const pull = (changes: Partial<OllamaPull>): OllamaPull => ({ model: "qwen3.8", status: "pulling manifest", completed: 0, total: 0, error: null, ...changes });

test("a size reads in gigabytes, as Ollama counts them, or in megabytes below one", () => {
  expect(sizeLabel(18_158_563_016)).toBe("18.2 GB");
  expect(sizeLabel(181_324_349)).toBe("181 MB");
});

test("a download shows its percent only once Ollama gives its size", () => {
  expect(pullLabel(pull({}))).toBe("pulling manifest");
  expect(pullPercent(pull({ status: "pulling abc", completed: 21, total: 42 }))).toBe(50);
  expect(pullLabel(pull({ status: "pulling abc", completed: 21, total: 42 }))).toBe("pulling abc, 50%");
  expect(pullPercent(pull({ completed: 50, total: 42 }))).toBe(100);
});

test("a failed download says why", () => {
  expect(pullLabel(pull({ error: "pull model manifest: file does not exist" }))).toBe("Failed: pull model manifest: file does not exist");
});

test("installing Ollama reads as its download, then its check and unpacking, or why it failed", () => {
  const step = { phase: "downloading", completed: 90_662_175, total: 181_324_350, error: null };
  expect(installLabel(step)).toBe("Downloading Ollama, 50% of 181 MB");
  expect(installLabel({ ...step, phase: "checking" })).toBe("Checking the download");
  expect(installLabel({ ...step, phase: "unpacking" })).toBe("Unpacking Ollama");
  expect(installLabel({ ...step, error: "the download does not match its published checksum" })).toBe("Could not install Ollama: the download does not match its published checksum");
});
