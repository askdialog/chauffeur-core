import type { FileDiff, FileStatus, RepoDiff, WorkspaceInfo } from "@chauffeur/protocol";
import { ChevronDown, ChevronRight, FileDiff as FileDiffIcon, GitBranch, Pencil, RefreshCw } from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type DiffHunk, keepUnchangedFiles, keepUnchangedRepos, languageOf, type PatchFile, parsePatch } from "@/lib/diff";
import { highlightLines } from "@/lib/highlight";
import { errorMessage, isNotImplemented } from "@/lib/rpc";
import { client, openFile } from "@/state/app";
import { useWorkspaceSelector, type WorkspaceSession } from "@/state/session";
import { Button, cx, EmptyState, IconButton, Spinner } from "./ui";

type Loaded = { repos: RepoDiff[]; at: number };

export function DiffPanel({ workspace, session }: { workspace: WorkspaceInfo; session: WorkspaceSession }) {
  const turnsEnded = useWorkspaceSelector(session, (s) => s.turnsEnded);
  const worktreesChanged = useWorkspaceSelector(session, (s) => s.worktreesChanged);
  const [data, setData] = useState<Loaded | null>(null);
  const [error, setError] = useState<{ message: string; notImplemented: boolean } | null>(null);
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);
  const scroller = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    const current = ++generation.current;
    setLoading(true);
    try {
      const result = await client.call("diff.get", { workspace: workspace.id, stat_only: false });
      if (current !== generation.current) return;
      // Read again at each turn's end: what did not change stays as drawn.
      setData((shown) => {
        const repos = keepUnchangedRepos(shown?.repos ?? null, result.repos);
        return shown && repos === shown.repos ? shown : { repos, at: Date.now() };
      });
      setError(null);
    } catch (err) {
      if (current !== generation.current) return;
      setError({ message: errorMessage(err), notImplemented: isNotImplemented(err) });
    } finally {
      if (current === generation.current) setLoading(false);
    }
  }, [workspace.id]);

  // On open, when a turn ends and when worktrees change.
  useEffect(() => {
    if (workspace.state === "archived") return;
    void load();
  }, [load, turnsEnded, worktreesChanged, workspace.state]);

  if (workspace.state === "archived") {
    return (
      <EmptyState icon={<FileDiffIcon size={22} />} title="Archived">
        The worktrees of an archived workspace are removed; restore it to see its diff.
      </EmptyState>
    );
  }

  const totals = data?.repos.reduce(
    (acc, repo) => {
      for (const file of repo.files) {
        acc.files += 1;
        acc.add += file.additions;
        acc.del += file.deletions;
      }
      return acc;
    },
    { files: 0, add: 0, del: 0 },
  );

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex w-[280px] shrink-0 flex-col border-r border-line">
        <div className="flex h-9 shrink-0 items-center gap-2 border-b border-line px-3 text-[12px] text-muted">
          {totals ? (
            <span>
              {totals.files} file{totals.files === 1 ? "" : "s"}{" "}
              <span className="text-success">+{totals.add}</span> <span className="text-danger">−{totals.del}</span>
            </span>
          ) : (
            <span>Changes</span>
          )}
          <Button size="sm" variant="ghost" className="ml-auto" onClick={() => void load()} title="Refresh">
            <RefreshCw size={12} className={loading ? "animate-spin" : ""} />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto py-1">
          {data?.repos.map((repo) => (
            <div key={repo.worktree_id} className="mb-2">
              <div className="flex items-center gap-1.5 px-3 py-1 text-[11.5px]">
                <span className="font-medium text-fg">{repo.repo}</span>
                <span className="truncate text-faint" title={`${repo.branch || "detached"} ← ${repo.base_branch}`}>
                  {repo.base_branch}
                </span>
                {repo.ahead > 0 && <span className="ml-auto shrink-0 text-faint">↑{repo.ahead}</span>}
              </div>
              {repo.files.length === 0 && <div className="px-3 text-[11.5px] text-faint">No changes</div>}
              {repo.files.map((file) => (
                <button
                  key={file.path}
                  onClick={() =>
                    scroller.current
                      ?.querySelector(`[data-file="${CSS.escape(`${repo.repo}/${file.path}`)}"]`)
                      ?.scrollIntoView({ block: "start" })
                  }
                  className="flex w-full items-center gap-1.5 px-3 py-0.5 text-left text-[12px] hover:bg-hover"
                  title={file.old_path ? `${file.old_path} → ${file.path}` : file.path}
                >
                  <StatusLetter status={file.status} />
                  <span className="min-w-0 flex-1 truncate text-muted">
                    <span className="text-faint">{dirname(file.path)}</span>
                    {basename(file.path)}
                  </span>
                  <span className="shrink-0 text-[11px] text-success">{file.additions ? `+${file.additions}` : ""}</span>
                  <span className="shrink-0 text-[11px] text-danger">{file.deletions ? `−${file.deletions}` : ""}</span>
                </button>
              ))}
            </div>
          ))}
        </div>
      </aside>
      <div ref={scroller} className="min-h-0 min-w-0 flex-1 overflow-auto">
        {error ? (
          <EmptyState icon={<FileDiffIcon size={22} />} title={error.notImplemented ? "Diff is not available yet" : "Could not load the diff"}>
            {error.notImplemented
              ? "The daemon does not implement diff.get yet. It will appear here once it does."
              : error.message}
          </EmptyState>
        ) : !data ? (
          <div className="flex h-full items-center justify-center text-faint">
            <Spinner />
          </div>
        ) : totals?.files === 0 ? (
          <EmptyState icon={<FileDiffIcon size={22} />} title="No changes">
            The worktrees match their base branches.
          </EmptyState>
        ) : (
          <div className="space-y-4 p-4">
            {data.repos.map((repo) => (
              <RepoSection key={repo.worktree_id} repo={repo} workspaceId={workspace.id} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

const RepoSection = memo(function RepoSection({ repo, workspaceId }: { repo: RepoDiff; workspaceId: string }) {
  // A file whose lines did not change keeps its object, so that its section is not highlighted again.
  const parsed = useRef<PatchFile[]>([]);
  const files = useMemo(() => (parsed.current = keepUnchangedFiles(parsed.current, parsePatch(repo.patch))), [repo.patch]);
  if (repo.files.length === 0) return null;
  return (
    <section>
      <div className="mb-2 flex items-center gap-2 text-[12.5px]">
        <span className="font-semibold text-fg">{repo.repo}</span>
        <GitBranch size={12} className="text-faint" />
        <span className="font-mono text-[11.5px] text-muted">
          {repo.branch || "detached"} ← {repo.base_branch}
        </span>
        {repo.ahead > 0 && (
          <span className="text-[11.5px] text-faint">
            {repo.ahead} commit{repo.ahead === 1 ? "" : "s"} ahead
          </span>
        )}
        {repo.truncated && <span className="text-[11.5px] text-attention">patch truncated</span>}
      </div>
      <div className="space-y-2">
        {repo.files.map((file) => (
          <FileSection
            key={file.path}
            workspaceId={workspaceId}
            repo={repo.repo}
            file={file}
            patch={files.find((p) => p.path === file.path)}
          />
        ))}
      </div>
    </section>
  );
});

const COLLAPSE_LINES = 600;

const FileSection = memo(function FileSection({
  workspaceId,
  repo,
  file,
  patch,
}: {
  workspaceId: string;
  repo: string;
  file: FileDiff;
  patch: PatchFile | undefined;
}) {
  const lineCount = patch?.hunks.reduce((n, h) => n + h.lines.length, 0) ?? 0;
  const [open, setOpen] = useState(lineCount <= COLLAPSE_LINES && file.status !== "deleted");
  const language = languageOf(file.path);
  return (
    <div className="overflow-hidden rounded-md border border-line" data-file={`${repo}/${file.path}`}>
      <div className="sticky top-0 z-[1] flex items-center border-b border-line bg-raised pr-1.5">
        <button
          onClick={() => setOpen((v) => !v)}
          className="flex min-w-0 flex-1 items-center gap-2 px-3 py-1.5 text-left text-[12px]"
        >
          {open ? <ChevronDown size={12} className="text-faint" /> : <ChevronRight size={12} className="text-faint" />}
          <StatusLetter status={file.status} />
          <span className="truncate font-mono text-[11.5px] text-fg">
            {file.old_path ? `${file.old_path} → ` : ""}
            {file.path}
          </span>
          <span className="ml-auto shrink-0 text-[11px] text-success">+{file.additions}</span>
          <span className="shrink-0 text-[11px] text-danger">−{file.deletions}</span>
        </button>
        {file.status !== "deleted" && !file.binary && (
          <IconButton
            title="Open in Files, to read or edit it"
            onClick={() => openFile(workspaceId, `${repo}/${file.path}`, firstChangedLine(patch))}
            data-testid="diff-open-file"
          >
            <Pencil size={12} />
          </IconButton>
        )}
      </div>
      {open &&
        (file.binary || patch?.binary ? (
          <div className="px-3 py-2 text-[12px] text-faint">Binary file</div>
        ) : !patch || patch.hunks.length === 0 ? (
          <div className="px-3 py-2 text-[12px] text-faint">
            {(file.status === "untracked" || file.status === "added") && file.additions === 0
              ? "New empty file"
              : "No textual changes"}
          </div>
        ) : (
          <div className="overflow-x-auto bg-[var(--code-bg)] font-mono text-[length:var(--code-size)] leading-[1.5] selectable">
            {patch.hunks.map((hunk, index) => (
              <Hunk key={index} hunk={hunk} language={language} />
            ))}
          </div>
        ))}
    </div>
  );
});

function Hunk({ hunk, language }: { hunk: DiffHunk; language: string | null }) {
  const html = useMemo(() => highlightLines(hunk.lines.map((l) => l.text), language), [hunk, language]);
  return (
    <div>
      <div className="bg-accent/[0.07] px-3 text-faint">{hunk.header}</div>
      <table className="w-full border-collapse">
        <tbody>
          {hunk.lines.map((line, index) => (
            <tr
              key={index}
              className={cx(
                line.type === "add" && "bg-success/[0.09]",
                line.type === "del" && "bg-danger/[0.09]",
              )}
            >
              <td className="w-[1%] select-none whitespace-nowrap px-2 text-right text-faint/70">{line.oldNo ?? ""}</td>
              <td className="w-[1%] select-none whitespace-nowrap px-2 text-right text-faint/70">{line.newNo ?? ""}</td>
              <td
                className={cx(
                  "w-[1%] select-none pl-1 pr-1",
                  line.type === "add" && "text-success",
                  line.type === "del" && "text-danger",
                )}
              >
                {line.type === "add" ? "+" : line.type === "del" ? "−" : " "}
              </td>
              <td className="whitespace-pre pr-4 text-[var(--code-fg)]">
                {line.type === "meta" ? (
                  <span className="text-faint">{line.text}</span>
                ) : (
                  <span dangerouslySetInnerHTML={{ __html: html[index] || " " }} />
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** The first added line of the file in the new version, to open it there. */
function firstChangedLine(patch: PatchFile | undefined): number | null {
  for (const hunk of patch?.hunks ?? []) {
    const line = hunk.lines.find((l) => l.type === "add" && l.newNo != null);
    if (line) return line.newNo;
  }
  return patch?.hunks[0]?.lines.find((l) => l.newNo != null)?.newNo ?? null;
}

function StatusLetter({ status }: { status: FileStatus }) {
  const map: Record<FileStatus, [string, string]> = {
    added: ["A", "text-success"],
    untracked: ["U", "text-success"],
    modified: ["M", "text-attention"],
    deleted: ["D", "text-danger"],
    renamed: ["R", "text-working"],
  };
  const [letter, color] = map[status];
  return <span className={cx("w-3 shrink-0 text-center font-mono text-[11px] font-semibold", color)}>{letter}</span>;
}

function dirname(path: string): string {
  const index = path.lastIndexOf("/");
  return index >= 0 ? path.slice(0, index + 1) : "";
}

function basename(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}
