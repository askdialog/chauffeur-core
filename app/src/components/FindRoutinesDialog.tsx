// Find routines (docs/adr/find-routines.md): starts an analysis of the user's own messages of the
// last days with the agent and model chosen, and shows what it proposes as cards: the requests a
// routine comes from, with a link to their chat, the routine written properly, and Create, Later
// or Ignore. The cards stay, behind the sidebar's Suggestions entry, until acted on.

import type { RoutineSuggestion } from "@chauffeur/protocol";
import { ChevronRight, ExternalLink, Repeat, Sparkles } from "lucide-react";
import { useEffect, useState } from "react";
import { effectiveLoadout, initialChoice, type ModelChoice } from "@/lib/models";
import { scheduleText } from "@/lib/routines";
import {
  createSuggestedRoutine,
  findRoutines,
  loadLoadout,
  loadModels,
  selectChat,
  selectWorkspace,
  setSuggestion,
  showFindRoutines,
  useApp,
} from "@/state/app";
import { ModelPicker } from "./ModelPicker";
import { Badge, Button, cx, inputClass, Modal, Spinner } from "./ui";

const DEFAULT_DAYS = 14;
/** What the last analysis was started with, for the next one in this session. */
let last: { days: string; choice: ModelChoice | null } = { days: String(DEFAULT_DAYS), choice: null };

export function FindRoutinesDialog() {
  const shown = useApp((s) => s.findRoutinesShown);
  if (!shown) return null;
  return <Dialog />;
}

function Dialog() {
  const find = useApp((s) => s.routineFind);
  const saved = useApp((s) => s.loadout);
  const catalog = useApp((s) => s.models);
  const [days, setDays] = useState(last.days);
  const [choice, setChoice] = useState<ModelChoice | null>(last.choice);
  const [starting, setStarting] = useState(false);
  useEffect(() => {
    if (!catalog) void loadModels();
    if (!saved) void loadLoadout();
  }, [catalog, saved]);
  // The composer's first favorite, as a new chat would start.
  const model = choice ?? initialChoice(effectiveLoadout(saved, catalog));
  const count = Number(days);
  const valid = Number.isSafeInteger(count) && count >= 1 && count <= 90;
  const running = find?.running ?? null;
  const start = async () => {
    if (!valid || running || starting) return;
    setStarting(true);
    last = { days, choice: model };
    await findRoutines({ days: count, agent: model.agent, model: model.model ?? undefined, effort: model.effort ?? undefined });
    setStarting(false);
  };
  const suggestions = find?.suggestions ?? [];
  return (
    <Modal open onClose={() => showFindRoutines(false)} title="Find routines" width={760} height={640}>
      <div className="flex h-full min-h-0 flex-col gap-3 text-[12.5px]" data-testid="find-routines">
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void start();
          }}
          className="space-y-2"
        >
          <div className="flex flex-wrap items-center gap-2 text-muted">
            <span>Read my messages of the last</span>
            <div className="w-16">
              <input
                className={cx(inputClass, "text-center")}
                type="number"
                min={1}
                max={90}
                step={1}
                value={days}
                onChange={(event) => setDays(event.target.value)}
                disabled={!!running}
                aria-label="Days to read"
                data-testid="find-routines-days"
              />
            </div>
            <span>days with</span>
            <div className="min-w-[220px]" data-testid="find-routines-model">
              <ModelPicker field value={model} onChange={setChoice} disabled={!!running} />
            </div>
            <Button
              type="submit"
              variant="primary"
              busy={starting}
              disabled={!valid || !!running}
              data-testid="find-routines-start"
            >
              <Sparkles size={13} /> Find routines
            </Button>
          </div>
          <p className="text-[11.5px] text-faint">
            Reads only what you wrote, in every workspace, archived ones included, then proposes a routine for a request that comes
            back worded differently, or that names a time window (“the last 3 days”, “every morning”). Nothing runs until you create
            a routine and activate it.
          </p>
        </form>
        {running ? (
          <div className="flex items-center gap-2 rounded-md border border-line bg-panel px-3 py-2 text-muted" data-testid="find-routines-running">
            <Spinner size={12} />
            <span>
              {running.agent === "codex" ? "Codex" : "Claude"} reads {running.messages} of your messages of the last {running.days} days. This takes a
              minute or two; you can close this window.
            </span>
          </div>
        ) : find?.last_error ? (
          <p className="text-danger selectable">The last analysis failed: {find.last_error}</p>
        ) : find?.last_found === 0 && suggestions.length === 0 ? (
          <p className="text-muted">The last analysis found nothing worth a routine.</p>
        ) : null}
        <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pr-1">
          {suggestions.map((suggestion) => (
            <SuggestionCard key={suggestion.id} suggestion={suggestion} />
          ))}
        </div>
      </div>
    </Modal>
  );
}

function SuggestionCard({ suggestion }: { suggestion: RoutineSuggestion }) {
  const [promptShown, setPromptShown] = useState(false);
  const [busy, setBusy] = useState(false);
  const spec = suggestion.spec;
  const act = async (action: () => Promise<void>) => {
    setBusy(true);
    await action();
    setBusy(false);
  };
  const openChat = (workspace: string, chat: string) => {
    showFindRoutines(false);
    selectWorkspace(workspace);
    selectChat(workspace, chat);
  };
  return (
    <section className="rounded-lg border border-line bg-panel p-3" data-testid="routine-suggestion" data-suggestion={spec.name}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="flex items-center gap-1.5 font-medium text-fg">
            <Repeat size={12} className="shrink-0 text-faint" />
            <span className="truncate">{spec.name}</span>
            {suggestion.state === "later" && <Badge>later</Badge>}
          </h3>
          {suggestion.why && <p className="mt-0.5 text-muted">{suggestion.why}</p>}
        </div>
        <div className="shrink-0 text-right text-[11.5px] text-muted">
          <div>{scheduleText(spec.schedule)}</div>
          <div className="text-faint">{spec.repos.join(", ")}</div>
        </div>
      </div>
      <ul className="mt-2 space-y-1">
        {suggestion.requests.map((request) => (
          <li key={`${request.chat_id}-${request.at}`} className="flex items-start gap-2 rounded-md bg-bg px-2 py-1.5">
            <div className="min-w-0 flex-1">
              <div className="line-clamp-2 whitespace-pre-wrap text-fg selectable">{request.text}</div>
              <div className="mt-0.5 flex flex-wrap items-center gap-1.5 text-[11px] text-faint">
                <span>{new Date(request.at).toLocaleString(undefined, { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" })}</span>
                <span>·</span>
                <span className="truncate">{request.workspace}</span>
                {request.window && <Badge tone="accent" title="The time window it names gives the cadence">{request.window}</Badge>}
              </div>
            </div>
            <Button size="xs" variant="ghost" title="Open the chat it was written in" onClick={() => openChat(request.workspace_id, request.chat_id)}>
              <ExternalLink size={11} /> Chat
            </Button>
          </li>
        ))}
      </ul>
      <button
        type="button"
        className="mt-2 flex items-center gap-1 text-[11.5px] text-muted hover:text-fg"
        aria-expanded={promptShown}
        onClick={() => setPromptShown(!promptShown)}
      >
        <ChevronRight size={12} className={cx("transition-transform", promptShown && "rotate-90")} /> Proposed prompt
      </button>
      {promptShown && (
        <pre className="mt-1 max-h-60 overflow-y-auto whitespace-pre-wrap rounded-md bg-bg p-2 font-sans text-[12px] text-fg selectable">{spec.prompt}</pre>
      )}
      <div className="mt-2 flex items-center justify-end gap-1.5">
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => void act(() => setSuggestion(suggestion, "ignored"))} title="Never propose it again">
          Ignore
        </Button>
        {suggestion.state !== "later" && (
          <Button size="sm" variant="ghost" disabled={busy} onClick={() => void act(() => setSuggestion(suggestion, "later"))} title="Keep it among the suggestions">
            Later
          </Button>
        )}
        <Button
          size="sm"
          variant="primary"
          busy={busy}
          onClick={() => void act(() => createSuggestedRoutine(suggestion))}
          title="A draft routine in a workspace of its own, with this proposal as its first version: run it by hand before activating it"
          data-testid="create-suggested-routine"
        >
          Create the routine
        </Button>
      </div>
    </section>
  );
}
