// GitHub's state, shared by every place that shows the setup checklist: what one changes (an
// organization picked in the creation dialog) the others show at once.

import { useEffect } from "react";
import type { GithubStatus } from "@chauffeur/protocol";
import { create } from "zustand";
import { isConnecting } from "@/lib/github";
import { errorMessage } from "@/lib/rpc";
import { client, loadRepos } from "./app";

const useGithub = create<{ status: GithubStatus | null; error: string | null }>(() => ({ status: null, error: null }));

/** Reads GitHub's state; once the account or the organization changed, the repos a workspace
 *  picks from are listed again. */
export async function refreshGithub(): Promise<void> {
  try {
    const status = await client.call("github.status", {});
    const before = useGithub.getState().status;
    useGithub.setState({ status, error: null });
    if (before && (before.account !== status.account || before.organization !== status.organization)) void loadRepos(true);
  } catch (error) {
    useGithub.setState({ error: errorMessage(error) });
  }
}

/** GitHub's state, asked again every second while something moves (a connection, git's
 *  installer) and whenever the window comes back to the front, from GitHub's page say. */
export function useGithubStatus(): { status: GithubStatus | null; error: string | null } {
  const { status, error } = useGithub();
  const moving = !!status && (isConnecting(status) || !status.git);
  useEffect(() => {
    void refreshGithub();
    const onFocus = () => void refreshGithub();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);
  useEffect(() => {
    if (!moving) return;
    const timer = setInterval(() => void refreshGithub(), 1000);
    return () => clearInterval(timer);
  }, [moving]);
  return { status, error };
}

/** The GitHub account signed in, as last read (the creation dialog's checklist reads it). */
export function useGithubAccount(): string | null {
  return useGithub((s) => s.status?.account ?? null);
}
