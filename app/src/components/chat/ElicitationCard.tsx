import type { JsonValue, RequestResponse } from "@chauffeur/protocol";
import { useState } from "react";
import { ShieldQuestion } from "lucide-react";
import type { ElicitationItem } from "@/lib/chat";
import { elicitationFields, parseElicitationJson } from "@/lib/elicitation";
import { openExternal } from "@/lib/native";
import { Button, cx } from "../ui";

const inputClass = "rounded border border-line bg-panel px-2 py-1 text-fg";

export function ElicitationCard({ item, readOnly, respond }: {
  item: ElicitationItem;
  readOnly: boolean;
  respond: (id: string, response: RequestResponse) => Promise<void>;
}) {
  const [content, setContent] = useState<Record<string, JsonValue>>({});
  const [editJson, setEditJson] = useState(false);
  const [json, setJson] = useState("{}");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fields = editJson ? null : elicitationFields(item.schema);
  const parsed = fields ? { content, error: null } : parseElicitationJson(json);
  const open = item.state === "open" && !readOnly;
  const openPage = async () => {
    if (!item.url || item.error) return;
    setError(null);
    try { await openExternal(item.url); } catch { setError("Could not open the confirmation page; copy the URL into your browser."); }
  };
  const answer = async (action: "accept" | "decline" | "cancel", always = false) => {
    setBusy(true);
    setError(null);
    try {
      await respond(item.requestId, { type: "elicitation", action, content: action === "accept" && !item.url ? parsed.content : null, always });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Could not answer the confirmation; try again.");
    } finally {
      setBusy(false);
    }
  };
  return (
    <section data-testid="mcp-elicitation" data-request-state={item.state} className={cx("my-2 rounded-lg border px-3 py-2.5", open ? "border-attention/40 bg-attention/[0.06]" : "border-line bg-panel")}>
      <div className="flex items-center gap-2 font-medium text-fg"><ShieldQuestion size={14} />{item.serverName} requests confirmation</div>
      <p className="my-2 whitespace-pre-wrap break-words text-muted selectable">{item.message}</p>
      {item.error && <p role="alert" className="my-2 text-danger">{item.error}</p>}
      {open && !item.error && !item.url && fields !== null && <Button size="sm" variant="ghost" onClick={() => { setJson(JSON.stringify(content, null, 2)); setEditJson(true); }}>Edit JSON</Button>}
      {open && !item.error && (item.url ? (
        <Button onClick={openPage}>
          Open confirmation page
        </Button>
      ) : fields ? fields.map(([name, field]) => (
        <label key={name} className="my-2 flex flex-col gap-1 text-muted">
          <span>{typeof field.title === "string" ? field.title : name}</span>
          {typeof field.description === "string" && <span className="text-[11.5px] text-faint">{field.description}</span>}
          {field.type === "boolean" || Array.isArray(field.enum) ? (
            <select aria-label={name} className={inputClass} disabled={busy} value={Object.hasOwn(content, name) ? JSON.stringify(content[name]) : ""} onChange={(e) => {
              const next = { ...content };
              if (e.target.value === "") delete next[name];
              else next[name] = JSON.parse(e.target.value);
              setContent(next);
            }}>
              <option value="">Choose…</option>
              {(field.type === "boolean" ? [true, false] : Array.isArray(field.enum) ? field.enum : []).map((value) => <option key={JSON.stringify(value)} value={JSON.stringify(value)}>{String(value)}</option>)}
            </select>
          ) : (
            <input aria-label={name} className={inputClass} disabled={busy} type={field.type === "string" ? "text" : "number"} step={field.type === "integer" ? 1 : "any"} onChange={(e) => {
              const next = { ...content };
              if (e.target.value === "") delete next[name];
              else next[name] = field.type === "string" ? e.target.value : Number(e.target.value);
              setContent(next);
            }} />
          )}
        </label>
      )) : (
        <label className="my-2 flex flex-col gap-1 text-muted">Response (JSON)
          <textarea aria-label="Response (JSON)" rows={5} className={`${inputClass} font-mono`} value={json} disabled={busy} onChange={(e) => setJson(e.target.value)} />
        </label>
      ))}
      {open && !fields && !item.url && <details className="my-2 text-muted"><summary>Requested form</summary><pre className="overflow-auto text-[11.5px]">{JSON.stringify(item.schema, null, 2)}</pre></details>}
      {item.url && <p className="my-2 break-all font-mono text-[11.5px] text-muted selectable">
        {item.error ? item.url : <a href={item.url} target="_blank" rel="noopener noreferrer" className="text-accent underline" onClick={async (event) => {
          event.preventDefault();
          await openPage();
        }}>{item.url}</a>}
      </p>}
      {item.rules && (
        <p className="my-2 text-[11.5px] text-faint" data-testid="request-always">
          Not asked again in this chat: <span className="font-mono text-muted selectable">{item.rules.join(", ")}</span>
        </p>
      )}
      {(error || (open && !item.url && !item.error && parsed.error)) && <p role="alert" className="my-2 text-danger">{error || parsed.error}</p>}
      {open ? <>
        <div className="mt-2 flex gap-1.5">
          <Button variant="primary" size="sm" disabled={busy || !!item.error || (!item.url && !!parsed.error)} onClick={() => answer("accept")}>{item.url ? "Continue" : "Accept"}</Button>
          {item.rule && <Button size="sm" disabled={busy || !!item.error || !!parsed.error} onClick={() => answer("accept", true)}>Always allow</Button>}
          <Button variant="danger" size="sm" disabled={busy} onClick={() => answer("decline")}>Decline</Button>
          <Button size="sm" disabled={busy} onClick={() => answer("cancel")}>Cancel</Button>
        </div>
      </> : <p className="text-[11.5px] text-faint">{item.state === "expired" ? "Expired" : item.rules ? "Always allowed" : item.outcome ?? "Answered"}</p>}
    </section>
  );
}
