// The forge an installation goes through, GitHub unless set to a GitLab
// (docs/adr/gitlab-as-the-forge.md), and the words that follow it.

import type { Forge, GitlabConnecting } from "@chauffeur/protocol";
import { sizeLabel } from "./localModels";

/** What a change proposed for merging is called on this forge: "PR" or "MR", in full "pull
 *  request" or "merge request". */
export function changeRequestTerm(forge: Forge | undefined, form: "short" | "long" = "short"): string {
  const gitlab = forge === "gitlab";
  if (form === "long") return gitlab ? "merge request" : "pull request";
  return gitlab ? "MR" : "PR";
}

export function forgeName(forge: Forge | undefined): string {
  return forge === "gitlab" ? "GitLab" : "GitHub";
}

/** The words of the forge: its name, what a change proposed for merging is called, and how one is
 *  numbered (`#12` on GitHub, `!12` on GitLab). */
export type ForgeWords = { name: string; short: string; shorts: string; long: string; number: (n: number | bigint | null | undefined) => string };

export function forgeWords(forge: Forge | undefined): ForgeWords {
  const gitlab = forge === "gitlab";
  const short = changeRequestTerm(forge);
  return {
    name: forgeName(forge),
    short,
    shorts: `${short}s`,
    long: changeRequestTerm(forge, "long"),
    number: (n) => `${gitlab ? "!" : "#"}${n ?? ""}`,
  };
}

export function gitlabConnectingLabel(step: GitlabConnecting): string {
  if (step.error) return `Could not connect GitLab: ${step.error}`;
  if (step.phase === "downloading" && step.total > 0) {
    return `Getting GitLab's connector, ${Math.min(100, Math.floor((step.completed * 100) / step.total))}% of ${sizeLabel(step.total)}`;
  }
  if (step.phase === "downloading" || step.phase === "checking" || step.phase === "unpacking") return "Getting GitLab's connector";
  return "Signing in…";
}
