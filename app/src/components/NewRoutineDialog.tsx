// A routine made by hand (docs/adr/routines-by-hand.md), from the sidebar's "Routines" section: its
// name, its repos, what each run should do in the user's words, and the agent and model. The
// daemon makes the draft and starts its refining chat, which writes the prompt from the
// description and asks when it should run.

import { Plus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { effectiveLoadout, initialChoice, type ModelChoice } from "@/lib/models";
import { createRoutineByHand, loadLoadout, loadModels, showNewRoutine, useApp } from "@/state/app";
import { RepoChip } from "./CreateWorkspaceDialog";
import { ModelPicker } from "./ModelPicker";
import { RepoPopover } from "./RepoPicker";
import { Button, Field, inputClass, Modal } from "./ui";

export function NewRoutineDialog() {
  const shown = useApp((s) => s.newRoutineShown);
  if (!shown) return null;
  return <Dialog />;
}

function Dialog() {
  const saved = useApp((s) => s.loadout);
  const catalog = useApp((s) => s.models);
  const [name, setName] = useState("");
  const [repos, setRepos] = useState<string[]>([]);
  const [description, setDescription] = useState("");
  const [choice, setChoice] = useState<ModelChoice | null>(null);
  const [picking, setPicking] = useState(false);
  const [busy, setBusy] = useState(false);
  const repoButton = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!catalog) void loadModels();
    if (!saved) void loadLoadout();
  }, [catalog, saved]);
  // The composer's first favorite, as a new chat would start.
  const model = choice ?? initialChoice(effectiveLoadout(saved, catalog));
  const valid = name.trim() !== "" && repos.length > 0 && description.trim() !== "";
  const toggleRepo = (key: string) => setRepos((current) => (current.includes(key) ? current.filter((r) => r !== key) : [...current, key]));
  const close = () => showNewRoutine(false);
  const create = async () => {
    if (!valid || busy) return;
    setBusy(true);
    const made = await createRoutineByHand({
      name: name.trim(),
      repos,
      description: description.trim(),
      agent: model.agent,
      model: model.model ?? undefined,
      effort: model.effort ?? undefined,
    });
    if (!made) setBusy(false);
  };
  return (
    <Modal
      open
      onClose={close}
      title="New routine"
      width={560}
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy} disabled={!valid || busy} onClick={() => void create()} data-testid="new-routine-create">
            Create
          </Button>
        </>
      }
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void create();
        }}
        className="space-y-3 text-[12.5px]"
        data-testid="new-routine"
      >
        <Field label="Name">
          <input
            className={inputClass}
            autoFocus
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="Morning triage"
            disabled={busy}
            data-testid="new-routine-name"
          />
        </Field>
        <Field label="Repos" hint="each run works in them">
          <div className="flex flex-wrap items-center gap-1.5">
            {repos.map((key) => (
              <RepoChip key={key} name={key} onRemove={() => toggleRepo(key)} />
            ))}
            <span ref={repoButton}>
              <Button type="button" size="sm" variant="ghost" onClick={() => setPicking(!picking)} disabled={busy} data-testid="new-routine-add-repo">
                <Plus size={12} /> Add repo
              </Button>
            </span>
          </div>
          {picking && <RepoPopover anchor={repoButton} onClose={() => setPicking(false)} selection={repos} onToggle={toggleRepo} />}
        </Field>
        <Field label="What should each run do?" hint="in your words; the agent writes the prompt">
          <textarea
            className={`${inputClass} h-28 resize-none py-1.5`}
            value={description}
            onChange={(event) => setDescription(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                event.preventDefault();
                void create();
              }
            }}
            placeholder="Every weekday morning, triage the new Linear tickets and tell me which need me."
            disabled={busy}
            data-testid="new-routine-description"
          />
        </Field>
        <Field label="Agent">
          <div className="max-w-[280px]" data-testid="new-routine-model">
            <ModelPicker field value={model} onChange={setChoice} disabled={busy} />
          </div>
        </Field>
        <p className="text-[11.5px] text-faint">
          It starts as a draft: its refining chat rewrites the prompt and asks when it should run. Nothing runs by itself until you activate it.
        </p>
      </form>
    </Modal>
  );
}
