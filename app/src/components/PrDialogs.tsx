// The workspace's pull requests: opened by its agent (with open-pr and shepherd-pr skills when
// installed), merged by the user, one PR at a time.

import type { MergeMethod, PrInfo, WorkspaceInfo } from "@chauffeur/protocol";
import { CheckCircle2, CircleDashed, CircleX, XCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { commonMethods, MERGE_METHODS, prLabel, stackedAbove } from "@/lib/prs";
import { errorMessage } from "@/lib/rpc";
import { client, selectChat, setTab, toast, upsertChats } from "@/state/app";
import { Button, cx, inputClass, Modal } from "./ui";

export function CreatePrDialog({ open, onClose, workspace }: { open: boolean; onClose: () => void; workspace: WorkspaceInfo }) {
  const [instructions, setInstructions] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const chat = await client.call("pr.create", {
        workspace: workspace.id,
        draft: false,
        instructions: instructions.trim() || undefined,
      });
      upsertChats([chat]);
      selectChat(workspace.id, chat.id);
      setTab(workspace.id, "chat");
      onClose();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Create PRs"
      width={520}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy} onClick={() => void submit()}>
            Ask the agent
          </Button>
        </>
      }
    >
      <div className="space-y-3 text-[12.5px] text-muted">
        <p>
          The agent opens one PR per repo that has commits, with an <span className="font-mono text-fg">open-pr</span>{" "}
          skill when one is installed (else the repo's conventions). It links them together, then follows CI and the AI
          review with a <span className="font-mono text-fg">shepherd-pr</span> skill when there is one. It never merges.
        </p>
        <textarea
          className={cx(inputClass, "h-auto resize-y py-1.5")}
          rows={3}
          placeholder="Extra instructions (optional)"
          value={instructions}
          onChange={(e) => setInstructions(e.target.value)}
        />
        {error && <div className="text-danger selectable">{error}</div>}
      </div>
    </Modal>
  );
}

const METHOD_KEY = "chauffeur.mergeMethod";


/** Merges one pull request, with the method the user picks among those its repo allows. It cannot
 * be closed while the merge runs. */
export function MergeDialog({
  pr,
  prs,
  onClose,
  workspace,
  onMerged,
}: {
  pr: PrInfo | null;
  /** The workspace's PRs: the ones stacked on `pr` get a warning. */
  prs: PrInfo[];
  onClose: () => void;
  workspace: WorkspaceInfo;
  onMerged: () => void;
}) {
  const [method, setMethod] = useState<MergeMethod>(() => (localStorage.getItem(METHOD_KEY) as MergeMethod | null) ?? "squash");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => setError(null), [pr?.repo, pr?.branch]);

  const allowed = pr ? commonMethods([pr]) : [];
  const effective = allowed.includes(method) ? method : (allowed[0] ?? method);
  const above = pr ? stackedAbove(pr, prs) : [];

  const merge = async () => {
    if (!pr) return;
    setBusy(true);
    setError(null);
    localStorage.setItem(METHOD_KEY, effective);
    try {
      const target = { repo: pr.repo, branch: pr.branch };
      const result = await client.call("pr.merge", { workspace: workspace.id, method: effective, prs: [target] });
      onMerged();
      const outcome = result.outcomes.find((o) => o.repo === target.repo && o.branch === target.branch);
      if (outcome?.merged) {
        toast(`Merged: ${prLabel(pr)} #${pr.number}`);
        onClose();
      } else {
        setError(outcome?.error ?? "GitHub did not merge it");
      }
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={pr != null}
      onClose={onClose}
      dismissable={!busy}
      title="Merge pull request"
      width={560}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="success" busy={busy} onClick={() => void merge()} data-testid="confirm-merge">
            Merge #{pr ? String(pr.number) : ""}
          </Button>
        </>
      }
    >
      {pr && (
        <div className="space-y-3 text-[12.5px]">
          <div className="rounded-md border border-line bg-raised px-2.5 py-1.5">
            <span className="flex items-baseline gap-1.5">
              <span className="shrink-0 whitespace-nowrap font-medium text-fg">{prLabel(pr)}</span>
              <span className="shrink-0 text-faint">#{pr.number}</span>
              <span className="truncate text-muted">{pr.title}</span>
            </span>
            <span className="mt-0.5 flex flex-wrap gap-x-3 text-[11.5px]">
              <Readiness pr={pr} />
            </span>
          </div>
          <div className="flex flex-wrap gap-2" role="radiogroup" aria-label="Merge method">
            {MERGE_METHODS.filter((m) => allowed.includes(m.id)).map((m) => (
              <button
                key={m.id}
                type="button"
                role="radio"
                aria-checked={effective === m.id}
                onClick={() => setMethod(m.id)}
                className={cx(
                  "flex flex-col rounded-md border px-2.5 py-1.5 text-left",
                  effective === m.id ? "border-accent/60 bg-accent/10" : "border-line hover:border-line-strong",
                )}
                data-testid={`merge-method-${m.id}`}
              >
                <span className={cx("text-[12.5px] font-medium", effective === m.id ? "text-fg" : "text-muted")}>{m.label}</span>
                <span className="text-[11px] text-faint">{m.detail}</span>
              </button>
            ))}
          </div>
          {above.length > 0 && effective !== "merge" && (
            <p className="text-[11.5px] text-attention" data-testid="stacked-warning">
              {above.map((p) => `#${p.number}`).join(", ")} {above.length === 1 ? "is" : "are"} stacked on it: once it is{" "}
              {effective === "squash" ? "squashed" : "rebased"}, {above.length === 1 ? "that PR still carries" : "they still carry"} its
              original commits, so GitHub may report conflicts. A merge commit avoids it.
            </p>
          )}
          {error && (
            <div className="flex items-start gap-2 rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-danger selectable">
              <XCircle size={14} className="mt-0.5 shrink-0" />
              <span>{error}</span>
            </div>
          )}
        </div>
      )}
    </Modal>
  );
}

/** What GitHub will look at: checks, review, conflicts, draft. */
function Readiness({ pr }: { pr: PrInfo }) {
  const checks =
    pr.checks === "SUCCESS" ? (
      <span className="inline-flex items-center gap-1 text-success">
        <CheckCircle2 size={11} /> checks passed
      </span>
    ) : pr.checks === "FAILURE" ? (
      <span className="inline-flex items-center gap-1 text-danger">
        <CircleX size={11} /> checks failed
      </span>
    ) : pr.checks === "PENDING" ? (
      <span className="inline-flex items-center gap-1 text-attention">
        <CircleDashed size={11} /> checks running
      </span>
    ) : null;
  const review =
    pr.review_decision === "APPROVED"
      ? { text: "approved", tone: "text-success" }
      : pr.review_decision === "CHANGES_REQUESTED"
        ? { text: "changes requested", tone: "text-danger" }
        : pr.review_decision === "REVIEW_REQUIRED"
          ? { text: "review required", tone: "text-attention" }
          : null;
  return (
    <>
      {pr.is_draft && <span className="text-attention">draft</span>}
      {checks}
      {review && <span className={review.tone}>{review.text}</span>}
      {pr.mergeable === "CONFLICTING" && <span className="text-danger">conflicts with {pr.pr_base_branch ?? pr.base_branch}</span>}
    </>
  );
}
