import type { SectionInfo } from "@chauffeur/protocol";
import { Plus, X } from "lucide-react";
import { useRef, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import { client, createSection, toast } from "@/state/app";
import { RepoPopover } from "./RepoPicker";
import { Button, Modal } from "./ui";

export type SectionReposAction = "repo" | "group" | SectionInfo;

export function SectionReposDialog({ action, onClose }: { action: SectionReposAction; onClose: () => void }) {
  const editing = typeof action === "object";
  const single = action === "repo";
  const [repos, setRepos] = useState<string[]>(editing ? action.repos ?? [] : []);
  const [name, setName] = useState("");
  const [pickerOpen, setPickerOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const title = editing ? `Default repos for ${action.name}` : single ? "Add repository to sidebar" : "New repository group";
  const sectionName = single ? repos[0] : name.trim();
  const valid = editing || (!!sectionName && repos.length > 0);

  async function save() {
    if (!valid || saving) return;
    setSaving(true);
    try {
      if (editing) await client.call("section.set_repos", { section: action.id, repos });
      else if (!(await createSection(sectionName, undefined, repos))) return;
      onClose();
    } catch (error) {
      toast(`Could not update default repos: ${errorMessage(error)}`, "error");
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal open onClose={onClose} dismissable={!saving} title={title} width={500}>
      <form
        className="space-y-4 p-4"
        onSubmit={(event) => { event.preventDefault(); void save(); }}
      >
        {action === "group" && (
          <label className="flex flex-col gap-1.5 text-[12px] text-muted">
            Group name
            <input
              value={name}
              onChange={(event) => setName(event.target.value)}
              maxLength={60}
              autoFocus
              className="h-8 rounded-md border border-line bg-bg px-2 text-fg outline-none focus:border-accent"
            />
          </label>
        )}
        <div className="flex flex-wrap items-center gap-1.5">
          {repos.map((repo) => (
            <Button key={repo} type="button" size="sm" title={`Remove ${repo}`} onClick={() => setRepos((items) => items.filter((item) => item !== repo))}>
              {repo} <X size={11} />
            </Button>
          ))}
          <button
            type="button"
            ref={anchor}
            onClick={() => setPickerOpen(!pickerOpen)}
            aria-expanded={pickerOpen}
            className="inline-flex h-6 items-center gap-1 rounded-md border border-dashed border-line-strong px-2 text-xs text-muted hover:bg-hover hover:text-fg"
          >
            <Plus size={12} /> {single ? (repos.length > 0 ? "Change repo" : "Choose repo") : "Choose repos"}
          </button>
          {pickerOpen && (
            <RepoPopover
              anchor={anchor}
              onClose={() => setPickerOpen(false)}
              selection={repos}
              single={single}
              allowCreate={false}
              onToggle={(repo) => {
                setRepos((items) => items.includes(repo) ? items.filter((item) => item !== repo) : single ? [repo] : [...items, repo]);
                if (single) setPickerOpen(false);
              }}
            />
          )}
        </div>
        {editing && repos.length === 0 && <p className="text-xs text-muted">Add repos to enable one-click workspace creation from this section.</p>}
        <div className="flex justify-end gap-2">
          <Button type="button" onClick={onClose} disabled={saving}>Cancel</Button>
          <Button type="submit" variant="primary" busy={saving} disabled={!valid}>{editing ? "Save" : single ? "Add repository" : "Create group"}</Button>
        </div>
      </form>
    </Modal>
  );
}
