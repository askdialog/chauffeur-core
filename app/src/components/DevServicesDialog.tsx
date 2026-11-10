// A workspace's development services, from its menu: the containers detection finds in it, each
// declared as data that follows the workspace to another machine, a cache rebuilt there, or left
// out; and its web previews, the ports that answer HTTP, checked to declare. The daemon detects
// and saves; the dialog only shows and chooses.

import type { DetectedPreview, DetectedService, DevServices, WorkspaceInfo } from "@chauffeur/protocol";
import { AlertTriangle, Database, Globe, RefreshCw, XCircle } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import {
  allowedChoices,
  badPreviewNames,
  blockedChoices,
  devServicesCopy,
  initialChoices,
  initialPreviews,
  type PreviewDraft,
  savedChoices,
  savedPreviews,
  type ServiceChoice,
} from "@/lib/devServices";
import { workspaceName } from "@/lib/format";
import { errorMessage } from "@/lib/rpc";
import { client, toast } from "@/state/app";
import { Button, cx, Modal, Spinner } from "./ui";

export function DevServicesDialog({ workspace, onClose }: { workspace: WorkspaceInfo; onClose: () => void }) {
  const french = navigator.language.startsWith("fr"),
    copy = devServicesCopy(french);
  const [detected, setDetected] = useState<DetectedService[] | null>(null);
  const [declared, setDeclared] = useState<DevServices | null>(null);
  const [choices, setChoices] = useState<Record<string, ServiceChoice>>({});
  const [previews, setPreviews] = useState<DetectedPreview[]>([]);
  const [drafts, setDrafts] = useState<Record<number, PreviewDraft>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  const detect = useCallback(() => {
    setDetected(null);
    setFailed(false);
    setError(null);
    Promise.all([
      client.call("services.get", { workspace: workspace.id }),
      client.call("services.detect", { workspace: workspace.id }),
    ])
      .then(([current, found]) => {
        setDeclared(current);
        setDetected(found.services);
        setChoices(initialChoices(found.services, current));
        setPreviews(found.previews);
        setDrafts(initialPreviews(found.previews));
      })
      .catch((err) => {
        // Not an empty result: saving it would drop every declared service.
        setDetected([]);
        setPreviews([]);
        setFailed(true);
        setError(errorMessage(err));
      });
  }, [workspace.id]);
  useEffect(detect, [detect]);

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await client.call("services.save", {
        workspace: workspace.id,
        services: savedChoices(choices),
        revision: declared?.revision ?? null,
        previews: savedPreviews(drafts),
      });
      toast(copy.saved);
      onClose();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const labels: Record<ServiceChoice, string> = { data: copy.data, cache: copy.cache, skip: copy.skip };
  const blocked = detected ? blockedChoices(detected, choices) : [];
  const badNames = badPreviewNames(drafts);
  return (
    <Modal
      open
      onClose={onClose}
      title={copy.title(workspaceName(workspace))}
      width={560}
      footer={
        <>
          <Button variant="ghost" onClick={detect} disabled={detected === null || busy}>
            <RefreshCw size={12} /> {copy.detectAgain}
          </Button>
          <span className="flex-1" />
          <Button variant="ghost" onClick={onClose}>
            {copy.cancel}
          </Button>
          <Button
            variant="primary"
            onClick={() => void save()}
            busy={busy}
            disabled={
              detected === null ||
              failed ||
              blocked.length > 0 ||
              badNames.length > 0 ||
              (detected.length === 0 && !declared?.services.length && previews.length === 0)
            }
            data-testid="save-dev-services"
          >
            {copy.save}
          </Button>
        </>
      }
    >
      <div className="space-y-3 text-[12.5px] text-muted">
        <p>{copy.intro}</p>
        {detected === null && (
          <div className="flex items-center gap-2">
            <Spinner /> {copy.detecting}
          </div>
        )}
        {detected?.length === 0 && !error && <p>{copy.none}</p>}
        {detected?.map((service) => {
          const allowed = allowedChoices(service);
          const choice = choices[service.name] ?? "skip";
          return (
            <div key={service.name} className="rounded-md border border-line px-3 py-2" data-dev-service={service.name}>
              <div className="flex items-center gap-2">
                {service.postgres && <Database size={13} className="shrink-0 text-accent" />}
                <span className="font-medium text-fg">{service.name}</span>
                <span className="truncate font-mono text-[11px] text-faint">
                  {service.image}
                  {!service.running && ` · ${copy.stopped}`}
                  {service.compose ? ` · ${service.compose.file}` : ` · ${copy.noCompose}`}
                </span>
              </div>
              <div role="radiogroup" aria-label={service.name} className="mt-1.5 flex flex-wrap gap-1">
                {(["data", "cache", "skip"] as const).map((option) => (
                  <label
                    key={option}
                    className={cx(
                      "flex items-center gap-1.5 rounded border px-2 py-1",
                      !allowed.includes(option) && "cursor-not-allowed opacity-40",
                      allowed.includes(option) && "cursor-pointer",
                      choice === option ? "border-accent/50 bg-accent/5 text-fg" : "border-line",
                    )}
                    title={
                      option === "data"
                        ? copy.dataHint(service.postgres?.databases ?? [])
                        : option === "cache"
                          ? copy.cacheHint
                          : copy.skipHint
                    }
                  >
                    <input
                      type="radio"
                      name={`service-${service.name}`}
                      disabled={!allowed.includes(option)}
                      checked={choice === option}
                      onChange={() => setChoices({ ...choices, [service.name]: option })}
                    />
                    {labels[option]}
                  </label>
                ))}
              </div>
              {choice === "data" && service.postgres && (
                <p className="mt-1 text-[11.5px] text-faint">{copy.dataHint(service.postgres.databases)}</p>
              )}
              {service.problem && (
                <p className="mt-1 flex items-start gap-1.5 text-[11.5px] text-attention">
                  <AlertTriangle size={12} className="mt-0.5 shrink-0" />
                  {service.problem}
                </p>
              )}
            </div>
          );
        })}
        {blocked.map((name) => (
          <p key={name} className="text-attention">
            {copy.keepData(name)}
          </p>
        ))}
        {detected !== null && !failed && (
          <div className="space-y-1.5 pt-1">
            <div className="flex items-center gap-1.5 font-medium text-fg">
              <Globe size={13} className="text-accent" /> {copy.previews}
            </div>
            <p>{previews.length ? copy.previewsIntro : copy.noPreviews}</p>
            {previews.map((preview) => {
              const draft = drafts[preview.port] ?? { name: preview.name, declare: false };
              return (
                <div
                  key={preview.port}
                  className="flex items-center gap-2 rounded-md border border-line px-3 py-1.5"
                  data-preview={preview.port}
                >
                  <input
                    type="checkbox"
                    checked={draft.declare}
                    aria-label={copy.previewName(preview.port)}
                    onChange={() => setDrafts({ ...drafts, [preview.port]: { ...draft, declare: !draft.declare } })}
                  />
                  <input
                    className={cx(
                      "w-40 rounded border bg-transparent px-1.5 py-0.5 text-fg",
                      badNames.includes(preview.port) ? "border-danger/60" : "border-line",
                    )}
                    value={draft.name}
                    aria-label={copy.previewName(preview.port)}
                    onChange={(event) =>
                      setDrafts({ ...drafts, [preview.port]: { ...draft, name: event.target.value } })
                    }
                  />
                  <span className="font-mono text-[11px] text-faint">
                    localhost:{preview.port}
                    {!preview.listening && ` · ${copy.silent}`}
                  </span>
                </div>
              );
            })}
            {badNames.map((port) => (
              <p key={port} className="text-attention">
                {copy.badPreviewName(port)}
              </p>
            ))}
          </div>
        )}
        {error && (
          <div className="flex items-start gap-2 whitespace-pre-wrap rounded-md border border-danger/30 bg-danger/10 px-3 py-2 text-[12.5px] text-danger selectable">
            <XCircle size={14} className="mt-0.5 shrink-0" />
            <span>{error}</span>
          </div>
        )}
      </div>
    </Modal>
  );
}
