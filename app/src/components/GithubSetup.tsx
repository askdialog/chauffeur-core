import { type ReactNode, useState } from "react";
import { Check, ClipboardCopy, GitBranch } from "lucide-react";
import { client } from "@/state/app";
import { refreshGithub, useGithubStatus } from "@/state/github";
import { errorMessage } from "@/lib/rpc";
import { connectingLabel, githubStep, isConnecting, organizationChoices } from "@/lib/github";
import { openExternal } from "@/lib/native";
import { Button, cx, Spinner } from "./ui";

/**
 * Connecting GitHub for whoever has neither Homebrew nor GitHub CLI: git, then the account,
 * signed in in the browser, then the organization repos come from. Shows nothing once all is
 * done, unless `manage` (Settings › GitHub), which also offers to disconnect or change.
 */
export function GithubSetup({ manage = false, compact = false }: { manage?: boolean; compact?: boolean }) {
  const { status, error: loadError } = useGithubStatus();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [gitAsked, setGitAsked] = useState(false);
  const [changingOrg, setChangingOrg] = useState(false);
  const [copied, setCopied] = useState(false);
  const step = status ? githubStep(status) : null;

  async function run(action: () => Promise<unknown>, failure: string) {
    setBusy(true);
    setError(null);
    try {
      await action();
      await refreshGithub();
    } catch (err) {
      setError(`${failure}: ${errorMessage(err)}`);
    } finally {
      setBusy(false);
    }
  }

  async function copyAndOpen(code: string, url: string) {
    await navigator.clipboard.writeText(code).catch(() => {});
    setCopied(true);
    await openExternal(url);
  }

  if (!status) return loadError ? <p className="text-[12px] text-danger selectable">Could not read GitHub's state: {loadError}</p> : null;
  if (step === "ready" && !manage) return null;

  const connecting = status.connecting;
  const choosing = step === "organization" || changingOrg;
  const choices = organizationChoices(status);

  return (
    <div
      className={cx("w-full space-y-3 text-left", !manage && "rounded-lg border border-line bg-raised", !manage && (compact ? "p-3" : "max-w-[460px] p-4"))}
      data-testid="github-setup"
    >
      {!manage && (
        <div>
          <div className="mb-1 flex items-center gap-1.5 font-medium text-fg">
            <GitBranch size={14} /> Connect GitHub
          </div>
          {!compact && (
            <p className="text-[12px] leading-relaxed text-muted">
              Chauffeur works on your GitHub repositories. Connect your account once, in your browser: no terminal, no SSH key.
            </p>
          )}
        </div>
      )}

      <Item done={status.git} title="git" detail={status.git ? "Installed" : undefined}>
        {!status.git &&
          (status.git_install === "xcode" ? (
            gitAsked ? (
              <p className="flex items-center gap-2 text-[12px] text-muted">
                <Spinner /> In Apple's window, click Install, then wait a few minutes: this goes on by itself once it is done.
              </p>
            ) : (
              <div className="space-y-1.5">
                <p className="text-[12px] text-muted">Every workspace needs git. Apple installs it with its developer tools, in a few minutes.</p>
                <Button
                  variant="primary"
                  disabled={busy}
                  data-testid="install-git"
                  onClick={() => void run(async () => {
                    await client.call("git.install", {});
                    setGitAsked(true);
                  }, "Could not open Apple's installer")}
                >
                  Install git
                </Button>
              </div>
            )
          ) : (
            <p className="text-[12px] text-muted">
              Install git with <code className="rounded bg-hover px-1 font-mono text-fg selectable">{status.git_install}</code>, then come back here.
            </p>
          ))}
      </Item>

      <Item done={!!status.account} title="GitHub account" detail={status.account ?? undefined}>
        {status.account && manage && !isConnecting(status) && (
          <Button size="sm" disabled={busy} onClick={() => void run(() => client.call("github.disconnect", {}), "Could not disconnect GitHub")}>
            Disconnect
          </Button>
        )}
        {!status.account && status.git && !isConnecting(status) && (
          <Button variant="primary" disabled={busy} data-testid="connect-github" onClick={() => void run(() => client.call("github.connect", {}), "Could not connect GitHub")}>
            {connecting?.error ? "Try again" : "Connect GitHub"}
          </Button>
        )}
        {connecting && (
          <div className="space-y-2" data-testid="github-connecting">
            <p className={cx("flex items-center gap-2 text-[12px]", connecting.error ? "text-danger selectable" : "text-muted")}>
              {!connecting.error && !connecting.code && <Spinner />}
              {connectingLabel(connecting)}
            </p>
            {!connecting.error && connecting.phase === "downloading" && connecting.total > 0 && (
              <div className="h-1 overflow-hidden rounded bg-line">
                <div className="h-full bg-accent transition-[width]" style={{ width: `${Math.min(100, (connecting.completed * 100) / connecting.total)}%` }} />
              </div>
            )}
            {!connecting.error && connecting.code && (
              <div className="flex flex-wrap items-center gap-2">
                <code className="rounded-md border border-line-strong bg-hover px-3 py-1.5 font-mono text-[18px] tracking-[0.15em] text-fg selectable" data-testid="github-code">
                  {connecting.code}
                </code>
                <Button variant="primary" onClick={() => void copyAndOpen(connecting.code ?? "", connecting.url ?? "https://github.com/login/device")}>
                  <ClipboardCopy size={13} /> {copied ? "Copied: open GitHub again" : "Copy the code and open GitHub"}
                </Button>
              </div>
            )}
            {!connecting.error && (
              <Button size="sm" variant="ghost" onClick={() => void run(() => client.call("github.cancel", {}), "Could not cancel")}>
                Cancel
              </Button>
            )}
          </div>
        )}
      </Item>

      {status.account && (
        <Item done={!!status.organization && !changingOrg} title="Repositories" detail={status.organization ? `From ${status.organization}` : undefined}>
          {choosing ? (
            <div className="space-y-1.5">
              <p className="text-[12px] text-muted">Where do the repositories you work on live?</p>
              <div className="flex flex-wrap gap-1.5">
                {choices.map((org) => (
                  <Button
                    key={org}
                    size="sm"
                    variant={org === status.organization ? "primary" : "secondary"}
                    disabled={busy}
                    onClick={() => void run(async () => {
                      await client.call("settings.set", { workspaces: { github_org: org } });
                      setChangingOrg(false);
                    }, `Could not choose ${org}`)}
                  >
                    {org === status.account ? `${org} (your own)` : org}
                  </Button>
                ))}
              </div>
            </div>
          ) : (
            manage && (
              <Button size="sm" disabled={busy} onClick={() => setChangingOrg(true)}>
                Change
              </Button>
            )
          )}
        </Item>
      )}

      {error && <p className="text-[12px] text-danger selectable">{error}</p>}
    </div>
  );
}

function Item({ done, title, detail, children }: { done: boolean; title: string; detail?: string; children?: ReactNode }) {
  return (
    <div className="flex gap-2.5">
      <span
        className={cx(
          "mt-[1px] flex h-4 w-4 shrink-0 items-center justify-center rounded-full border",
          done ? "border-success bg-success/20 text-success" : "border-line-strong",
        )}
      >
        {done && <Check size={10} />}
      </span>
      <div className="min-w-0 flex-1 space-y-1.5">
        <div className="flex items-baseline gap-2 text-[12.5px]">
          <span className="font-medium text-fg">{title}</span>
          {detail && <span className="truncate text-muted">{detail}</span>}
        </div>
        {children}
      </div>
    </div>
  );
}
