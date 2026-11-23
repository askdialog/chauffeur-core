import type { SecurityAudit, SecurityAuditStatus } from "@chauffeur/protocol";
import { FileText, ShieldCheck } from "lucide-react";
import { useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { errorMessage } from "@/lib/rpc";
import { client, toast } from "@/state/app";
import { Badge, Button, Modal, Spinner } from "./ui";

const STATUS: Record<SecurityAuditStatus, string> = {
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  canceled: "Canceled",
  interrupted: "Interrupted",
};

const PHASE: Record<string, string> = {
  preflight: "Preparing",
  threat_model: "Threat model",
  discovery: "Discovery",
  validation: "Validation",
  attack_path: "Attack path analysis",
  reducing: "Consolidating results",
  reporting: "Reporting",
};

const UNIT: Record<string, string> = {
  passes: "passes",
  checks: "checks",
  threat_surfaces: "threat surfaces",
  review_receipts: "reviews",
  candidate_findings: "candidates",
  validated_findings: "findings",
  report_artifacts: "report artifacts",
};

const isActive = (scan: SecurityAudit) => scan.status === "running" || scan.status === "interrupted";
const timeFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "medium" });

export function SecurityAuditCard({ workspace }: { workspace: string }) {
  const [scans, setScans] = useState<SecurityAudit[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [checkedAt, setCheckedAt] = useState(0);
  const [showHistory, setShowHistory] = useState(false);
  const [report, setReport] = useState<{ scan: SecurityAudit; text?: string; error?: string } | null>(null);
  const [confirmation, setConfirmation] = useState<SecurityAudit | null>(null);
  const [sending, setSending] = useState(false);
  const [requestedAt, setRequestedAt] = useState<Record<string, number>>({});

  useEffect(() => {
    let closed = false;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      let interval = 15_000;
      try {
        const result = await client.call("security_audit.list", { workspace });
        if (closed) return;
        setScans(result);
        setCheckedAt(Date.now());
        setError(null);
        if (result.some(isActive)) interval = 5_000;
      } catch (err) {
        if (!closed) setError(errorMessage(err));
      } finally {
        if (!closed) timer = setTimeout(() => void refresh(), interval);
      }
    }
    void refresh();
    return () => {
      closed = true;
      clearTimeout(timer);
    };
  }, [workspace]);

  async function openReport(scan: SecurityAudit) {
    setReport({ scan });
    try {
      const text = await client.call("security_audit.report", { workspace, scan: scan.id });
      setReport((current) => current?.scan.id === scan.id ? { scan, text } : current);
    } catch (err) {
      setReport((current) => current?.scan.id === scan.id ? { scan, error: errorMessage(err) } : current);
    }
  }

  async function cancel(scan: SecurityAudit) {
    setSending(true);
    try {
      await client.call("security_audit.cancel", { workspace, scan: scan.id });
      setRequestedAt((current) => ({ ...current, [scan.id]: Date.now() }));
      setConfirmation(null);
      toast("Cancellation requested in the original Codex chat.");
    } catch (err) {
      toast(`Could not request cancellation: ${errorMessage(err)}`, "error");
    } finally {
      setSending(false);
    }
  }

  if (scans.length === 0) return null;
  const active = scans.filter(isActive);
  const visible = showHistory ? scans : active.length ? active : scans.slice(0, 1);

  return (
    <section aria-label="Codex Security audits" className="shrink-0 border-b border-line bg-panel px-4 py-2" data-testid="security-audit-card">
      <header className="flex items-center gap-2 text-[12px] font-medium text-fg">
        <ShieldCheck size={14} className="text-accent" aria-hidden="true" />
        Codex Security
        {scans.length > (active.length || 1) && (
          <Button size="sm" variant="ghost" aria-expanded={showHistory} onClick={() => setShowHistory(!showHistory)} className="ml-auto">
            {showHistory ? "Hide history" : "Audit history"}
          </Button>
        )}
      </header>
      {error && <p role="status" className="mt-1 text-[11.5px] text-muted">Could not refresh audit status. Showing the last update.</p>}
      <ul className="mt-1 max-h-40 space-y-1 overflow-y-auto">
        {visible.map((scan) => {
          const when = new Date(scan.last_activity_at);
          const stale = scan.status === "running" && scan.mode === "deep" &&
            checkedAt - new Date(scan.heartbeat_at ?? scan.last_activity_at).getTime() > 120_000;
          const cancellationPending = checkedAt - (requestedAt[scan.id] ?? 0) < 30_000;
          return (
            <li key={scan.id} className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[11.5px]">
              <span className="font-medium text-fg">{scan.repo}</span>
              <Badge>{scan.mode === "deep" ? "Deep" : scan.mode === "diff" ? "Diff" : "Standard"}</Badge>
              <Badge tone={stale ? "attention" : scan.status === "running" ? "working" : scan.status === "completed" ? "success" : "neutral"}>
                {stale ? "Awaiting activity" : STATUS[scan.status]}
              </Badge>
              <span className="text-muted">{PHASE[scan.phase] ?? scan.phase}</span>
              {scan.progress && (
                <span className="text-muted" aria-label="Audit progress">
                  {scan.progress.completed}/{scan.progress.total} {UNIT[scan.progress.unit] ?? "items"}
                </span>
              )}
              {Number.isFinite(when.getTime()) && (
                <time dateTime={scan.last_activity_at} className="text-faint">Last activity {timeFormat.format(when)}</time>
              )}
              <div className="ml-auto flex items-center gap-1">
                {scan.has_report && (
                  <Button size="sm" onClick={() => void openReport(scan)}><FileText size={12} /> Open report</Button>
                )}
                {isActive(scan) && scan.owner_chat && (
                  <Button size="sm" variant="danger" disabled={!!error || cancellationPending} onClick={() => setConfirmation(scan)}>
                    {cancellationPending ? "Cancellation requested" : "Cancel audit"}
                  </Button>
                )}
              </div>
            </li>
          );
        })}
      </ul>
      {confirmation && (
        <Modal
          open
          title="Cancel security audit?"
          onClose={() => setConfirmation(null)}
          dismissable={!sending}
          footer={<>
            <Button disabled={sending} onClick={() => setConfirmation(null)}>Keep audit</Button>
            <Button variant="danger" busy={sending} onClick={() => void cancel(confirmation)}>Request cancellation</Button>
          </>}
        >
          <p className="text-sm text-muted">Send a cancellation request for <strong>{confirmation.repo}</strong> to the Codex chat that started it.</p>
        </Modal>
      )}
      {report && (
        <Modal open title={`Security audit · ${report.scan.repo}`} onClose={() => setReport(null)} width={850} height={720}>
          {report.error ? (
            <p role="alert" className="text-sm text-muted">Could not open the audit report. {report.error}</p>
          ) : report.text == null ? <Spinner /> : (
            <div className="md selectable">
              <ReactMarkdown remarkPlugins={[remarkGfm]} disallowedElements={["img", "a"]} unwrapDisallowed skipHtml>
                {report.text}
              </ReactMarkdown>
            </div>
          )}
        </Modal>
      )}
    </section>
  );
}
