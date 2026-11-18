import type { WorkspaceInfo } from "@chauffeur/protocol";
import { KeyRound } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { openExternal } from "@/lib/native";
import { sessionStateText } from "@/lib/sessions";
import { errorMessage } from "@/lib/rpc";
import { client, toast } from "@/state/app";
import { useWorkspaceSelector, type WorkspaceSession } from "@/state/session";
import { Button, cx } from "./ui";

/**
 * The logins the workspace's repos declare (`[sessions.<name>]` of `.chauffeur/settings.toml`),
 * beside its run scripts: whether each holds, and Log in, which opens the link the login prints.
 */
export function SessionsSection({ workspace, session }: { workspace: WorkspaceInfo; session: WorkspaceSession }) {
  const sessions = useWorkspaceSelector(session, (s) => s.sessions);
  const worktreesChanged = useWorkspaceSelector(session, (s) => s.worktreesChanged);
  const [loaded, setLoaded] = useState(false);
  const [loggingIn, setLoggingIn] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const result = await client.call("session.list", { workspace: workspace.id });
      session.listSessions(result.sessions);
    } catch (error) {
      toast(`Could not check the sessions: ${errorMessage(error)}`, "error");
    } finally {
      setLoaded(true);
    }
  }, [workspace.id, session]);

  useEffect(() => {
    void refresh();
  }, [refresh, worktreesChanged]);

  const login = async (name: string) => {
    setLoggingIn(name);
    try {
      const result = await client.call("session.login", { workspace: workspace.id, name });
      if (result.url) await openExternal(result.url);
      else if (result.output) toast(result.output, "info");
    } catch (error) {
      toast(`Could not start the login: ${errorMessage(error)}`, "error");
    } finally {
      setLoggingIn(null);
    }
  };

  const list = Object.values(sessions).sort((a, b) => a.name.localeCompare(b.name));
  if (!loaded || list.length === 0) return null;
  return (
    <div className="border-b border-line p-3" data-testid="sessions">
      <div className="mb-2 text-[11px] font-medium uppercase tracking-wide text-faint">Sessions</div>
      <div className="space-y-1">
        {list.map((item) => (
          <div key={item.name} className="flex items-center gap-2">
            <KeyRound size={12} className="shrink-0 text-faint" aria-hidden />
            <span className="min-w-0 flex-1 truncate text-[12px]" title={item.repo ? `${item.label} (${item.repo})` : item.label}>
              {item.label}
              <span className={cx("ml-1.5", item.state === "expired" ? "text-danger" : "text-faint")}>
                {sessionStateText(item)}
              </span>
            </span>
            {item.state !== "holds" && !item.logging_in && (
              <Button size="sm" onClick={() => void login(item.name)} busy={loggingIn === item.name}>
                Log in
              </Button>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
