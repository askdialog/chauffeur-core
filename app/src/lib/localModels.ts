// Settings › Local models (story 52): what to suggest, and how a size or a download reads.

import type { OllamaInstalling, OllamaPull } from "@chauffeur/protocol";

/** The models tried on a 48 GB Mac, best first; Ollama's names. */
export const SUGGESTED_MODELS = [
  { name: "qwen3.8", size: "18 GB", hint: "The best we tried: right on a small task, in about 16 minutes on an M5 Pro" },
  { name: "glm-4.7-flash", size: "19 GB", hint: "Faster, less reliable" },
] as const;

export function sizeLabel(bytes: number): string {
  return bytes < 1e9 ? `${Math.round(bytes / 1e6)} MB` : `${(bytes / 1e9).toFixed(1)} GB`;
}

/** How far a download is, in whole percent; null while Ollama gives no size. */
export function pullPercent(pull: OllamaPull): number | null {
  if (pull.total <= 0) return null;
  return Math.min(100, Math.floor((pull.completed * 100) / pull.total));
}

export function pullLabel(pull: OllamaPull): string {
  if (pull.error) return `Failed: ${pull.error}`;
  const percent = pullPercent(pull);
  return percent === null ? pull.status : `${pull.status}, ${percent}%`;
}

/** Where installing Ollama stands: its phase, and the download's percent. */
export function installLabel(step: OllamaInstalling): string {
  if (step.error) return `Could not install Ollama: ${step.error}`;
  if (step.phase === "downloading" && step.total > 0) return `Downloading Ollama, ${Math.min(100, Math.floor((step.completed * 100) / step.total))}% of ${sizeLabel(step.total)}`;
  if (step.phase === "checking") return "Checking the download";
  if (step.phase === "unpacking") return "Unpacking Ollama";
  return step.phase;
}
