// The create dialog's design, from the 3D view (docs/office.md): one of the user's design chats,
// or a colleague's shared with them or with the team, copied into the new workspace and named in
// its first prompt.

import type { SharedDesign } from "@chauffeur/protocol";
import { ChevronDown, Loader2, PenTool, X } from "lucide-react";
import { useEffect, useState } from "react";
import { relativeTime } from "@/lib/format";
import { type DesignOption, designOptions, designsOf } from "@/lib/designs";
import { errorMessage } from "@/lib/rpc";
import { client, loadChats, useApp } from "@/state/app";
import { cx, Field } from "@/components/ui";

/** A design of the user's or shared with them, copied into the new workspace and named to its agent. */
export function DesignPicker({ design, onPick }: { design: DesignOption | null; onPick: (design: DesignOption | null) => void }) {
  const [open, setOpen] = useState(false);
  const workspaces = useApp((s) => s.workspaces);
  const chats = useApp((s) => s.chats);
  const [shared, setShared] = useState<SharedDesign[] | null>(null);
  const [sharedError, setSharedError] = useState<string | null>(null);

  useEffect(() => {
    if (open) void Promise.all(Object.keys(workspaces).map((workspace) => loadChats(workspace)));
  }, [open]);

  useEffect(() => {
    if (!open || shared || sharedError) return;
    let live = true;
    client
      .call("design.shared", {})
      .then((result) => live && setShared(result.designs))
      .catch((e) => live && setSharedError(`Could not read your colleagues' designs: ${errorMessage(e)}`));
    return () => {
      live = false;
    };
  }, [open, shared, sharedError]);
  const options = designOptions(designsOf(Object.values(chats), Object.values(workspaces)), shared ?? []);
  const pick = (option: DesignOption) => {
    onPick(option);
    setOpen(false);
  };

  return (
    <Field label="Design" hint="optional: its screens copied into the workspace and named to the agent">
      {design ? (
        <div className="flex min-h-7 items-center gap-1.5 rounded-md border border-line bg-bg px-2 py-1" data-testid="create-design">
          <span className="text-[#d97757]">
            <PenTool size={12} />
          </span>
          <span className="min-w-0 truncate text-[12.5px] text-fg">{design.title}</span>
          <span className="shrink-0 text-[11.5px] text-faint">{design.detail}</span>
          <button type="button" aria-label="Remove the design" className="ml-auto shrink-0 rounded p-px text-faint hover:bg-hover hover:text-fg" onClick={() => onPick(null)}>
            <X size={11} />
          </button>
        </div>
      ) : (
        <div className="rounded-md border border-line bg-bg">
          <button
            type="button"
            className="flex h-7 w-full items-center gap-1.5 px-2 text-left text-[12.5px] text-faint hover:text-fg"
            aria-expanded={open}
            onClick={() => setOpen((o) => !o)}
            data-testid="create-design-add"
          >
            <PenTool size={12} /> Add a design…
            <ChevronDown size={12} className={cx("ml-auto transition-transform", open && "rotate-180")} />
          </button>
          {open && (
            <div className="max-h-[180px] overflow-y-auto border-t border-line py-1" role="listbox" data-testid="create-design-list">
              <DesignSection label="Yours" options={options.yours} empty="No design chat yet." onPick={pick} />
              {sharedError ? (
                <div className="px-2.5 py-1 text-[12px] text-danger">{sharedError}</div>
              ) : !shared ? (
                <div className="flex items-center gap-1.5 px-2.5 py-1 text-[12px] text-faint">
                  <Loader2 size={11} className="animate-spin" /> Colleagues' designs…
                </div>
              ) : (
                <>
                  <DesignSection label="Shared with you" options={options.named} empty="Nobody shared a design with you." onPick={pick} />
                  <DesignSection label="Team" options={options.team} empty="No design open to the team." onPick={pick} />
                </>
              )}
            </div>
          )}
        </div>
      )}
    </Field>
  );
}

function DesignSection({ label, options, empty, onPick }: { label: string; options: DesignOption[]; empty: string; onPick: (option: DesignOption) => void }) {
  return (
    <section>
      <h4 className="px-2.5 pb-0.5 pt-1 text-[10.5px] font-semibold uppercase tracking-wide text-faint">
        {label} · {options.length}
      </h4>
      {options.length === 0 ? (
        <div className="px-2.5 py-1 text-[12px] text-faint">{empty}</div>
      ) : (
        options.map((option) => (
          <button
            type="button"
            role="option"
            aria-selected={false}
            key={option.key}
            className="flex w-full min-w-0 items-baseline gap-2 px-2.5 py-1 text-left hover:bg-hover"
            onClick={() => onPick(option)}
            data-testid="create-design-option"
          >
            <span className="min-w-0 truncate text-[12.5px] text-fg">{option.title}</span>
            <span className="shrink-0 text-[11.5px] text-faint">{option.detail}</span>
            <span className="ml-auto shrink-0 text-[11px] text-faint">{relativeTime(option.changedAt)}</span>
          </button>
        ))
      )}
    </section>
  );
}
