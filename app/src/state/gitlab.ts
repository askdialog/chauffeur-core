// The GitLab an installation set to it goes through, shared by every place that shows its setup.

import { useEffect } from "react";
import type { Forge, GitlabStatus } from "@chauffeur/protocol";
import { create } from "zustand";
import { errorMessage } from "@/lib/rpc";
import { client, loadRepos } from "./app";

const useGitlab = create<{ status: GitlabStatus | null; error: string | null }>(() => ({ status: null, error: null }));

export async function refreshGitlab(): Promise<void> {
  try {
    const status = await client.call("gitlab.status", {});
    const before = useGitlab.getState().status;
    useGitlab.setState({ status, error: null });
    if (before && (before.account !== status.account || before.host !== status.host)) void loadRepos(true);
  } catch (error) {
    useGitlab.setState({ error: errorMessage(error) });
  }
}

/** GitLab's state, asked again every second while a connection runs. */
export function useGitlabStatus(): { status: GitlabStatus | null; error: string | null } {
  const { status, error } = useGitlab();
  const moving = !!status?.connecting && !status.connecting.error;
  useEffect(() => {
    void refreshGitlab();
    const onFocus = () => void refreshGitlab();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);
  useEffect(() => {
    if (!moving) return;
    const timer = setInterval(() => void refreshGitlab(), 1000);
    return () => clearInterval(timer);
  }, [moving]);
  return { status, error };
}

/** The forge the installation goes through, as last read: GitHub until GitLab's state says
 *  otherwise. `refreshGitlab` runs at the app's start. */
export function useForge(): Forge {
  return useGitlab((s) => (s.status?.enabled ? "gitlab" : "github"));
}
