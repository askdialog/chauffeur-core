// The Codex CLI chats run, as `codex.status` reports it, in words.

import type { CodexCliStatus } from "@chauffeur/protocol";

/** Whether version `a` comes before `b`, by their numbers (`0.99.0` before `0.167.0`). */
export function olderVersion(a: string, b: string): boolean {
  const numbers = (version: string) => {
    const parts: number[] = [];
    for (const part of version.split(/[.+-]/)) {
      if (!/^\d+$/.test(part)) break;
      parts.push(Number(part));
    }
    return parts;
  };
  const [x, y] = [numbers(a), numbers(b)];
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    if (x[i] === undefined) return true;
    if (y[i] === undefined) return false;
    if (x[i] !== y[i]) return x[i]! < y[i]!;
  }
  return false;
}

/** A newer Codex is out than the one installed. */
export function codexOutdated(status: CodexCliStatus): boolean {
  return !!status.version && !!status.latest && olderVersion(status.version, status.latest);
}

/** One line on the installed Codex. */
export function codexStateLabel(status: CodexCliStatus): string {
  if (!status.path) return "Codex is not installed: no `codex` on the login shell's PATH.";
  if (status.updating) return status.version ? `Updating Codex ${status.version}…` : "Updating Codex…";
  if (!status.version) return "Codex is installed; its version could not be read.";
  if (codexOutdated(status)) return `Codex ${status.version} is installed; ${status.latest} is out.`;
  if (status.latest) return `Codex ${status.version} is installed, the latest.`;
  return `Codex ${status.version} is installed.`;
}
