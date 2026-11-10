// Connecting GitHub without Homebrew: what is left to do before a workspace can be created, and
// how a connection under way reads.

import type { GithubConnecting, GithubStatus } from "@chauffeur/protocol";
import { sizeLabel } from "./localModels";

/** What GitHub still needs, in order: git, an account, the organization repos come from. */
export type GithubStep = "git" | "connect" | "organization" | "ready";

export function githubStep(status: GithubStatus): GithubStep {
  if (!status.git) return "git";
  if (!status.account) return "connect";
  if (!status.organization) return "organization";
  return "ready";
}

/** A connection runs: not finished, not failed. */
export function isConnecting(status: GithubStatus): boolean {
  return !!status.connecting && !status.connecting.error;
}

/** Where a workspace's repos may come from: the account's organizations, then the account itself. */
export function organizationChoices(status: GithubStatus): string[] {
  return status.account ? [...status.organizations, status.account] : status.organizations;
}

export function connectingLabel(step: GithubConnecting): string {
  if (step.error) return `Could not connect GitHub: ${step.error}`;
  if (step.phase === "downloading" && step.total > 0) {
    return `Getting GitHub's connector, ${Math.min(100, Math.floor((step.completed * 100) / step.total))}% of ${sizeLabel(step.total)}`;
  }
  if (step.phase === "downloading" || step.phase === "checking" || step.phase === "unpacking") return "Getting GitHub's connector";
  if (step.phase === "waiting" && step.code) return "Enter this code on GitHub, then click Authorize";
  if (step.phase === "configuring") return "Almost done: setting up git";
  return "Opening GitHub…";
}
