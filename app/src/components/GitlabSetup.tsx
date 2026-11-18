import { useState } from "react";
import { Check, GitBranch } from "lucide-react";
import { client } from "@/state/app";
import { refreshGitlab, useGitlabStatus } from "@/state/gitlab";
import { errorMessage } from "@/lib/rpc";
import { gitlabConnectingLabel } from "@/lib/forge";
import { openExternal } from "@/lib/native";
import { Button, cx, inputClass, Spinner } from "./ui";

/**
 * Connecting the GitLab an installation is set to (docs/adr/gitlab-as-the-forge.md): a personal
 * access token pasted here goes to GitLab's CLI, installed first when missing, and is never kept by
 * Chauffeur. Shows nothing once connected, unless `manage` (Settings), which offers to disconnect.
 */
export function GitlabSetup({ manage = false, compact = false }: { manage?: boolean; compact?: boolean }) {
  const { status, error: loadError } = useGitlabStatus();
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run(action: () => Promise<unknown>, failure: string) {
    setBusy(true);
    setError(null);
    try {
      await action();
      await refreshGitlab();
    } catch (err) {
      setError(`${failure}: ${errorMessage(err)}`);
    } finally {
      setBusy(false);
    }
  }

  if (!status) return loadError ? <p className="text-[12px] text-danger selectable">Could not read GitLab's state: {loadError}</p> : null;
  if (!status.enabled || (status.account && !manage)) return null;
  const connecting = status.connecting;
  const running = !!connecting && !connecting.error;
  const tokenPage = `https://${status.host}/-/user_settings/personal_access_tokens?name=Chauffeur&scopes=api,write_repository`;

  return (
    <div
      className={cx("w-full space-y-3 text-left", !manage && "rounded-lg border border-line bg-raised", !manage && (compact ? "p-3" : "max-w-[460px] p-4"))}
      data-testid="gitlab-setup"
    >
      {!manage && (
        <div className="flex items-center gap-1.5 font-medium text-fg">
          <GitBranch size={14} /> Connect GitLab
        </div>
      )}
      <div className="flex gap-2.5">
        <span
          className={cx(
            "mt-[1px] flex h-4 w-4 shrink-0 items-center justify-center rounded-full border",
            status.account ? "border-success bg-success/20 text-success" : "border-line-strong",
          )}
        >
          {status.account && <Check size={10} />}
        </span>
        <div className="min-w-0 flex-1 space-y-1.5">
          <div className="flex items-baseline gap-2 text-[12.5px]">
            <span className="font-medium text-fg">GitLab account</span>
            <span className="truncate text-muted">{status.account ? `${status.account} on ${status.host}` : status.host}</span>
          </div>
          {status.account && manage && !running && (
            <Button size="sm" disabled={busy} onClick={() => void run(() => client.call("gitlab.disconnect", {}), "Could not disconnect GitLab")}>
              Disconnect
            </Button>
          )}
          {!status.account && !running && (
            <form
              className="space-y-1.5"
              onSubmit={(event) => {
                event.preventDefault();
                const pasted = token.trim();
                if (!pasted) return;
                void run(async () => {
                  await client.call("gitlab.connect", { token: pasted });
                  setToken("");
                }, "Could not connect GitLab");
              }}
            >
              <p className="text-[12px] text-muted">
                Create a personal access token with the <code className="font-mono">api</code> and{" "}
                <code className="font-mono">write_repository</code> scopes on{" "}
                <button type="button" className="text-accent hover:underline" onClick={() => void openExternal(tokenPage)}>
                  your GitLab
                </button>
                , then paste it here. It goes to GitLab's CLI, which keeps it in your keychain: Chauffeur does not store it.
              </p>
              <div className="flex gap-2">
                <input
                  type="password"
                  value={token}
                  onChange={(event) => setToken(event.target.value)}
                  placeholder="glpat-…"
                  autoComplete="off"
                  spellCheck={false}
                  className={cx(inputClass, "flex-1 font-mono")}
                  data-testid="gitlab-token"
                />
                <Button type="submit" variant="primary" disabled={busy || !token.trim()} data-testid="connect-gitlab">
                  {connecting?.error ? "Try again" : "Connect"}
                </Button>
              </div>
            </form>
          )}
          {connecting && (
            <div className="space-y-2" data-testid="gitlab-connecting">
              <p className={cx("flex items-center gap-2 text-[12px]", connecting.error ? "text-danger selectable" : "text-muted")}>
                {!connecting.error && <Spinner />}
                {gitlabConnectingLabel(connecting)}
              </p>
              {!connecting.error && connecting.phase === "downloading" && connecting.total > 0 && (
                <div className="h-1 overflow-hidden rounded bg-line">
                  <div className="h-full bg-accent transition-[width]" style={{ width: `${Math.min(100, (connecting.completed * 100) / connecting.total)}%` }} />
                </div>
              )}
            </div>
          )}
        </div>
      </div>
      {error && <p className="text-[12px] text-danger selectable">{error}</p>}
    </div>
  );
}
