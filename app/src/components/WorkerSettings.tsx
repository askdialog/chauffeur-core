// Settings → Transfers, GCP workers: the daemon creates, installs and deletes them in the
// background; this section lists them, read again while one changes, and asks before anything
// that costs or deletes.
import type { AgentUsage, WorkerInfo, WorkersResult } from "@chauffeur/protocol";
import { Plus, RotateCw, Trash2, XCircle } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { AGENT_NAMES } from "@/lib/models";
import { errorMessage } from "@/lib/rpc";
import { signedOutAgents, workerLoginCopy } from "@/lib/workerLogin";
import { beyondLimit, changing, hourly, inProgress, nextWorkerName, validWorkerName, workerCopy, workerStatus } from "@/lib/workers";
import { client, toast } from "@/state/app";
import { useWorkerLogins } from "@/state/workerLogins";
import { Badge, Button, cx, Field, IconButton, inputClass, Modal, Spinner } from "./ui";
import { WorkerSignIn } from "./WorkerSignIn";

export function WorkerSettings() {
  const french = navigator.language.startsWith("fr"),
    copy = workerCopy(french);
  const [result, setResult] = useState<WorkersResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState<WorkerInfo | null>(null);
  const [details, setDetails] = useState<string | null>(null);
  const load = useCallback(
    () =>
      client
        .call("worker.list", {})
        .then((next) => {
          setResult(next);
          setError(null);
        })
        .catch((err) => setError(errorMessage(err))),
    [],
  );
  useEffect(() => void load(), [load]);
  useEffect(() => {
    if (!result) return;
    const timer = setTimeout(() => void load(), changing(result.workers) ? 1500 : 10_000);
    return () => clearTimeout(timer);
  }, [result, load]);

  const retry = async (worker: WorkerInfo) => {
    try {
      await client.call("worker.create", { name: worker.name, beyond_limit: false });
      await load();
    } catch (err) {
      toast(`Could not create ${worker.name} again: ${errorMessage(err)}`, "error");
    }
  };

  return (
    <section className="space-y-3" data-testid="worker-settings">
      <div>
        <h3 className="text-sm font-medium text-fg">{copy.section}</h3>
        {result && <p className="mt-2 text-[12.5px] leading-relaxed text-muted">{copy.intro(result.project, result.zone)}</p>}
      </div>
      {error && (
        <div className="flex items-start gap-2 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-[12.5px] text-danger selectable">
          <XCircle size={14} className="mt-0.5 shrink-0" />
          <span>{error}</span>
        </div>
      )}
      {!result && !error && <Spinner />}
      {result && (
        <div className="space-y-1.5" data-testid="workers">
          {result.workers.length === 0 && <p className="text-[12.5px] text-faint">{copy.none}</p>}
          {result.workers.map((worker) => {
            const status = workerStatus(worker, copy);
            const working = inProgress(worker);
            return (
              <div key={worker.name} className="rounded-md border border-line px-3 py-2" data-testid={`worker-${worker.name}`}>
                <div className="flex items-center gap-2">
                  <span className="font-medium text-fg">{worker.name}</span>
                  <span className="text-[12px] text-muted">
                    {worker.machine_type} · {hourly(worker.hourly_usd, french)}
                  </span>
                  <Badge tone={status.tone}>
                    {working && <Spinner size={10} />}
                    {status.label}
                  </Badge>
                  <span className="flex-1" />
                  <button
                    type="button"
                    className="text-[11.5px] text-faint hover:text-muted"
                    onClick={() => setDetails(details === worker.name ? null : worker.name)}
                  >
                    {copy.details}
                  </button>
                  {worker.state === "failed" && (
                    <IconButton title={copy.retry} onClick={() => void retry(worker)}>
                      <RotateCw size={12} />
                    </IconButton>
                  )}
                  {!working && (
                    <IconButton title={copy.delete(worker.name)} onClick={() => setDeleting(worker)}>
                      <Trash2 size={12} />
                    </IconButton>
                  )}
                </div>
                {worker.error && <p className="mt-1 text-[12px] text-danger selectable">{worker.error}</p>}
                {worker.state === "ready" && worker.connected && <WorkerAgents worker={worker.name} french={french} />}
                {details === worker.name && (
                  <pre className="mt-2 max-h-40 overflow-y-auto whitespace-pre-wrap text-[11px] text-muted selectable">
                    {worker.log.join("\n")}
                  </pre>
                )}
              </div>
            );
          })}
          <Button size="sm" variant="ghost" disabled={!result.project.trim()} onClick={() => setCreating(true)} data-testid="create-worker">
            <Plus size={12} /> {copy.create}
          </Button>
        </div>
      )}
      {creating && result && (
        <CreateWorker
          result={result}
          french={french}
          onClose={() => setCreating(false)}
          onCreated={() => {
            setCreating(false);
            void load();
          }}
        />
      )}
      {deleting && (
        <DeleteWorker
          worker={deleting}
          french={french}
          onClose={() => setDeleting(null)}
          onDeleted={() => {
            setDeleting(null);
            void load();
          }}
        />
      )}
    </section>
  );
}

/** Whether each agent is signed in on the worker, with a sign-in from here for those that are not. */
function WorkerAgents({ worker, french }: { worker: string; french: boolean }) {
  const copy = workerLoginCopy(french);
  const [agents, setAgents] = useState<AgentUsage[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Read again once a login on it completes.
  const signedIn = useWorkerLogins(
    (s) => Object.entries(s.logins).filter(([key, login]) => key.startsWith(`${worker}/`) && login.step === "done").length,
  );
  useEffect(() => {
    client
      .call("worker.usage", { name: worker })
      .then((usage) => {
        setAgents(usage.agents);
        setError(null);
      })
      .catch((err) => setError(errorMessage(err)));
  }, [worker, signedIn]);
  if (error) return <p className="mt-1 text-[11.5px] text-faint selectable">{error}</p>;
  if (!agents) return null;
  const out = signedOutAgents(agents);
  return (
    <div className="mt-1.5 space-y-1.5 text-[12px]" data-testid={`worker-agents-${worker}`}>
      <p className="text-[11.5px] text-faint">
        {copy.agents}:{" "}
        {agents
          .map((u) => `${AGENT_NAMES[u.agent]} ${u.account === "signed_in" ? copy.signedIn : u.account === "signed_out" ? copy.signedOut : copy.unknown}`)
          .join(" · ")}
      </p>
      {out.map((agent) => (
        <WorkerSignIn key={agent} worker={worker} agent={agent} french={french} />
      ))}
    </div>
  );
}

/** The estimate comes before anything is created; the user confirms it. */
function CreateWorker({
  result,
  french,
  onClose,
  onCreated,
}: {
  result: WorkersResult;
  french: boolean;
  onClose: () => void;
  onCreated: () => void;
}) {
  const copy = workerCopy(french);
  const [name, setName] = useState(() => nextWorkerName(result.workers));
  const [type, setType] = useState(result.default_machine_type);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const picked = result.machine_types.find((m) => m.name === type);
  const beyond = beyondLimit(result);
  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      await client.call("worker.create", { name, machine_type: type, beyond_limit: beyond });
      toast(copy.created(name));
      onCreated();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      open
      onClose={onClose}
      title={copy.confirmTitle(name)}
      width={500}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {copy.cancel}
          </Button>
          <Button
            variant="primary"
            onClick={() => void create()}
            busy={busy}
            disabled={!validWorkerName(name) || !picked}
            data-testid="confirm-create-worker"
          >
            {copy.confirm}
          </Button>
        </>
      }
    >
      <div className="space-y-3 text-[12.5px] text-muted">
        <Field label={copy.name} hint={copy.nameHint}>
          <input
            value={name}
            onChange={(event) => setName(event.target.value.trim())}
            spellCheck={false}
            className={cx(inputClass, "font-mono text-[12px]")}
            data-testid="worker-name"
          />
        </Field>
        <Field label={copy.machineType}>
          <div role="radiogroup" aria-label={copy.machineType} className="space-y-1">
            {result.machine_types.map((m) => (
              <label
                key={m.name}
                className={cx(
                  "flex cursor-pointer items-center gap-2 rounded-md border px-3 py-1.5",
                  type === m.name ? "border-accent/50 bg-accent/5 text-fg" : "border-line hover:bg-hover/60",
                )}
              >
                <input type="radio" name="machine-type" checked={type === m.name} onChange={() => setType(m.name)} />
                <span className="font-medium">{m.name}</span>
                <span className="text-[11.5px] text-faint">
                  {m.vcpus} vCPU · {m.memory_gb} {french ? "Go" : "GB"}
                </span>
                <span className="flex-1" />
                <span className="text-[11.5px]">{hourly(m.hourly_usd, french)}</span>
              </label>
            ))}
          </div>
        </Field>
        {picked && (
          <p className="text-fg" data-testid="worker-estimate">
            {copy.estimate(picked, result.disk_gb, result.zone, result.project, hourly(picked.hourly_usd, french))}
            <span className="block text-[11.5px] text-faint">{copy.estimateNote(result.prices_as_of)}</span>
          </p>
        )}
        {beyond && <p className="text-attention">{copy.beyondLimit(result.workers.length, result.limit)}</p>}
        {error && (
          <div className="flex items-start gap-2 whitespace-pre-wrap rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-danger selectable">
            <XCircle size={14} className="mt-0.5 shrink-0" />
            <span>{error}</span>
          </div>
        )}
      </div>
    </Modal>
  );
}

function DeleteWorker({
  worker,
  french,
  onClose,
  onDeleted,
}: {
  worker: WorkerInfo;
  french: boolean;
  onClose: () => void;
  onDeleted: () => void;
}) {
  const copy = workerCopy(french);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await client.call("worker.delete", { name: worker.name });
      onDeleted();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Modal
      open
      onClose={onClose}
      title={copy.deleteTitle(worker.name)}
      width={460}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {copy.cancel}
          </Button>
          <Button variant="danger" onClick={() => void remove()} busy={busy} data-testid="confirm-delete-worker">
            {copy.deleteConfirm}
          </Button>
        </>
      }
    >
      <div className="space-y-3 text-[12.5px] text-muted">
        <p>{copy.deleteExplain(worker.instance)}</p>
        {error && (
          <div className="flex items-start gap-2 whitespace-pre-wrap rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-danger selectable">
            <XCircle size={14} className="mt-0.5 shrink-0" />
            <span>{error}</span>
          </div>
        )}
      </div>
    </Modal>
  );
}
