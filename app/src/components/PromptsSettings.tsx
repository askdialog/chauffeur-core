// Settings › Prompts (docs/adr/saved-prompts.md): the user's saved prompts, each a name and a text,
// kept by the daemon as one file per prompt in ~/.chauffeur/prompts/. Read again each time the
// section opens, so a file edited by hand shows its new text.

import type { Prompt } from "@chauffeur/protocol";
import { Plus, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { errorMessage, fieldOf } from "@/lib/rpc";
import { client, toast } from "@/state/app";
import { Button, Field, inputClass, Spinner } from "./ui";

export function PromptsSettings() {
  const [prompts, setPrompts] = useState<Prompt[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const section = useRef<HTMLDivElement>(null);
  // Where the keyboard goes once the list shows a save or a delete: the saved prompt, or New.
  const focusAfter = useRef<{ prompt: string | null } | null>(null);
  const load = () => {
    setLoadError(null);
    client.call("prompts.list", {}).then(setPrompts).catch((error: unknown) => setLoadError(errorMessage(error)));
  };
  useEffect(load, []);
  useEffect(() => {
    const target = focusAfter.current;
    if (!target || !section.current) return;
    focusAfter.current = null;
    const editor = [...section.current.querySelectorAll<HTMLElement>('[data-testid="prompt"]')].find((li) => li.dataset.prompt === target.prompt);
    (editor?.querySelector<HTMLElement>('[data-testid="prompt-name"]') ?? section.current.querySelector<HTMLElement>('[data-testid="prompt-new"]'))?.focus();
  }, [prompts, adding]);
  const changed = (list: Prompt[], prompt: string | null) => {
    focusAfter.current = { prompt };
    setPrompts(list);
  };
  return (
    <div ref={section} className="flex flex-col gap-4" data-testid="prompts-settings">
      <div className="flex items-start justify-between gap-3">
        <div>
          <div className="mb-1 font-medium text-fg">Prompts</div>
          <p className="text-[12px] leading-relaxed text-muted">
            Messages you send often, written once: a short reply or a long instruction. Each one is a Markdown file of ~/.chauffeur/prompts/, which you can also edit by hand.
          </p>
        </div>
        {/* Clear of the scrolling edge, so the outline Settings search draws around it shows whole. */}
        <Button size="sm" className="mr-1 mt-1" onClick={() => setAdding(true)} data-testid="prompt-new"><Plus size={13} /> New</Button>
      </div>
      {loadError ? (
        <div role="alert" className="flex items-center gap-2 text-[12px] text-danger">
          <span className="selectable">Could not read the prompts: {loadError}</span>
          <Button size="sm" onClick={load}>Retry</Button>
        </div>
      ) : !prompts ? (
        <Spinner />
      ) : (
        <ul className="flex flex-col gap-3">
          {adding && (
            <PromptEditor key="new" onSaved={(list, name) => { setAdding(false); changed(list, name); }} onDiscard={() => { focusAfter.current = { prompt: null }; setAdding(false); }} />
          )}
          {prompts.map((prompt) => (
            // Keyed by its text too: once saved, the editor starts again from what the daemon read.
            <PromptEditor key={`${prompt.name}\n${prompt.text}`} prompt={prompt} onSaved={changed} />
          ))}
          {!adding && prompts.length === 0 && (
            <li className="rounded-md border border-dashed border-line px-3 py-4 text-center text-[12px] text-muted" data-testid="prompts-empty">
              No prompt saved yet: New adds your first one.
            </li>
          )}
        </ul>
      )}
    </div>
  );
}

/** One prompt's name and text, or a new prompt's when `prompt` is absent. */
function PromptEditor({ prompt, onSaved, onDiscard }: {
  prompt?: Prompt;
  /** The prompts after a save or a delete, and the prompt saved (none after a delete). */
  onSaved: (list: Prompt[], name: string | null) => void;
  onDiscard?: () => void;
}) {
  const [name, setName] = useState(prompt?.name ?? "");
  const [text, setText] = useState(prompt?.text ?? "");
  const [error, setError] = useState<{ name: boolean; message: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const changed = name !== (prompt?.name ?? "") || text !== (prompt?.text ?? "");
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      // Saving a name the daemon has replaces that prompt's text: a new prompt never takes one.
      const taken = !prompt && (await client.call("prompts.list", {})).find((p) => p.name === name);
      if (taken) {
        setError({ name: true, message: `Another prompt is already named "${taken.name}": choose another name` });
        return;
      }
      const list = await client.call("prompts.save", { name, text, previous: prompt?.name });
      // The daemon drops a trailing newline: the editor shows the text as it now reads.
      setText(list.find((p) => p.name === name)?.text ?? text);
      onSaved(list, name);
    } catch (failure) {
      setError({ name: fieldOf(failure) === "name", message: errorMessage(failure) });
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    if (!prompt) return onDiscard?.();
    if (!window.confirm(`Delete the prompt "${prompt.name}"?`)) return;
    setBusy(true);
    try {
      onSaved(await client.call("prompts.delete", { name: prompt.name }), null);
    } catch (failure) {
      toast(`Could not delete the prompt "${prompt.name}": ${errorMessage(failure)}`, "error");
      setBusy(false);
    }
  };
  return (
    <li className="flex flex-col gap-2 rounded-md border border-line p-3" data-testid="prompt" data-prompt={prompt?.name ?? ""}>
      <Field label="Name">
        <input className={inputClass} value={name} onChange={(event) => { setName(event.target.value); if (error?.name) setError(null); }} placeholder="Carry on" autoFocus={!prompt} data-testid="prompt-name" />
        {error?.name && <p role="alert" className="mt-1 text-[11.5px] text-danger selectable" data-testid="prompt-name-error">{error.message}</p>}
      </Field>
      <Field label="Text">
        <textarea className={`${inputClass} h-auto resize-y py-1.5`} rows={Math.min(10, Math.max(3, text.split("\n").length))} value={text} onChange={(event) => setText(event.target.value)} placeholder="Agreed, carry on." data-testid="prompt-text" />
      </Field>
      <div className="flex items-center justify-end gap-2">
        {error && !error.name && <p role="alert" className="mr-auto text-[11.5px] text-danger selectable" data-testid="prompt-save-error">{error.message}</p>}
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => void remove()} data-testid="prompt-delete"><Trash2 size={12} /> {prompt ? "Delete" : "Discard"}</Button>
        <Button size="sm" variant="primary" disabled={!changed} busy={busy} onClick={() => void save()} data-testid="prompt-save">Save</Button>
      </div>
    </li>
  );
}
