// Signing one agent in on a worker from this Mac (lib/workerLogin.ts): a button, then the page to
// open again, the code to type there (Codex) or a field for the code it shows (Claude Code), and
// how it ended.

import type { AgentKind } from "@chauffeur/protocol";
import { CheckCircle2 } from "lucide-react";
import { useState } from "react";
import { AGENT_NAMES } from "@/lib/models";
import { openExternal } from "@/lib/native";
import { loginKey, workerLoginCopy } from "@/lib/workerLogin";
import { cancelWorkerLogin, sendLoginCode, signInOnWorker, useWorkerLogins } from "@/state/workerLogins";
import { Button, cx, inputClass, Spinner } from "./ui";

export function WorkerSignIn({ worker, agent, french }: { worker: string; agent: AgentKind; french: boolean }) {
  const copy = workerLoginCopy(french);
  const login = useWorkerLogins((s) => s.logins[loginKey(worker, agent)]);
  const [code, setCode] = useState("");
  const [refused, setRefused] = useState<string | null>(null);
  const name = AGENT_NAMES[agent];
  const start = () => {
    setCode("");
    setRefused(null);
    void signInOnWorker(worker, agent);
  };
  if (!login || login.step === "failed") {
    return (
      <div className="space-y-1" data-testid={`worker-sign-in-${agent}`}>
        {login && <p className="whitespace-pre-wrap text-danger">{copy.failed(name, worker, login.error)}</p>}
        <Button size="sm" onClick={start} data-testid={`worker-sign-in-${agent}-start`}>
          {login ? copy.retry : copy.signIn(name, worker)}
        </Button>
      </div>
    );
  }
  if (login.step === "starting") {
    return (
      <p className="flex items-center gap-1.5 text-muted" data-testid={`worker-sign-in-${agent}`}>
        <Spinner size={10} /> {copy.signIn(name, worker)}…
      </p>
    );
  }
  if (login.step === "done") {
    return (
      <p className="flex items-center gap-1.5 text-success" data-testid={`worker-sign-in-${agent}`} data-done>
        <CheckCircle2 size={12} /> {copy.done(name, worker)}
      </p>
    );
  }
  const submit = async () => {
    setRefused(null);
    setRefused(await sendLoginCode(worker, agent, code.trim()));
  };
  return (
    <div className="space-y-1.5 rounded-md border border-line bg-raised/60 px-2.5 py-2 text-fg" data-testid={`worker-sign-in-${agent}`}>
      <p className="font-medium">{copy.signIn(name, worker)}</p>
      <p className="text-muted">
        {copy.opened} {login.userCode && copy.enterCode(login.userCode)}
      </p>
      {login.userCode && (
        <div className="flex items-center gap-2">
          <code className="rounded bg-bg px-2 py-0.5 font-mono text-[14px] tracking-wider selectable" data-testid="worker-login-user-code">
            {login.userCode}
          </code>
          <Button size="xs" variant="ghost" onClick={() => void navigator.clipboard.writeText(login.userCode!)}>
            {copy.copy}
          </Button>
        </div>
      )}
      {login.asksCode && (
        <label className="block text-muted" htmlFor={`worker-login-code-${worker}-${agent}`}>
          {copy.pasteCode}
        </label>
      )}
      {login.asksCode && (
        <form
          className="flex items-center gap-1.5"
          onSubmit={(event) => {
            event.preventDefault();
            if (code.trim()) void submit();
          }}
        >
          <input
            id={`worker-login-code-${worker}-${agent}`}
            value={code}
            onChange={(event) => setCode(event.target.value)}
            placeholder={copy.codePlaceholder}
            spellCheck={false}
            autoComplete="off"
            autoFocus
            className={cx(inputClass, "h-6 flex-1 font-mono text-[12px]")}
            data-testid="worker-login-code"
          />
          <Button size="sm" variant="primary" type="submit" busy={login.sending} disabled={!code.trim()}>
            {copy.send}
          </Button>
        </form>
      )}
      {!login.asksCode && (
        <p className="flex items-center gap-1.5 text-faint">
          <Spinner size={10} /> {copy.waiting}
        </p>
      )}
      {refused && <p className="whitespace-pre-wrap text-danger">{copy.couldNotSend(refused)}</p>}
      <div className="flex gap-1">
        <Button size="sm" variant="ghost" onClick={() => void openExternal(login.url)}>
          {copy.reopen}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => cancelWorkerLogin(worker, agent)}>
          {copy.cancel}
        </Button>
      </div>
      <p className="text-[11px] text-faint">{copy.tokenNote}</p>
    </div>
  );
}
