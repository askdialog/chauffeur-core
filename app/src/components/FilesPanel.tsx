import type { FileContent, FilesSearchResult, SearchMatch, WorkspaceInfo } from "@chauffeur/protocol";
import {
  AlertTriangle,
  CaseSensitive,
  ChevronDown,
  ChevronRight,
  ExternalLink,
  Eye,
  File,
  FileCode,
  Folder,
  FolderOpen,
  FolderSearch,
  Lock,
  MessageSquare,
  Regex,
  Share2,
  RotateCcw,
  Save,
  Search,
} from "lucide-react";
import { lazy, memo, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ancestors, basename, buildTree, filterFiles, type TreeDir } from "@/lib/files";
import { errorMessage, RpcCallError } from "@/lib/rpc";
import { kbd } from "@/lib/keys";
import { client, loadFileIndex, revealFile, toast, useApp } from "@/state/app";
import { useWorkspaceSelector, type WorkspaceSession } from "@/state/session";
import type { GotoLine } from "./CodeEditor";
import { FileLinkContext, REVEAL_LABEL, useFileLinker, useFileMenu } from "./FileLinks";
import { useWorkspaceFileLinker } from "./WorkspaceFileLinker";
import { CommentedMarkdown, useFileComments, useFileShare } from "./FileComments";
import { ShareDialog } from "./design/ShareDialog";
import { Button, cx, EmptyState, IconButton, Kbd, Spinner } from "./ui";

const CodeEditor = lazy(() => import("./CodeEditor"));

type Buffer = {
  content: FileContent | null;
  loading: boolean;
  error: string | null;
  /** The edited text; null while it matches the file. */
  draft: string | null;
  /** The file changed on disk under unsaved edits. */
  diskChanged: boolean;
  /** A save was refused: the file changed since it was read. */
  conflict: boolean;
  saving: boolean;
};

const EMPTY_BUFFER: Buffer = {
  content: null,
  loading: false,
  error: null,
  draft: null,
  diskChanged: false,
  conflict: false,
  saving: false,
};

const CONFLICT = 409;

export function FilesPanel({
  workspace,
  session,
  visible,
}: {
  workspace: WorkspaceInfo;
  session: WorkspaceSession;
  /** Hidden, it stays mounted for its open files: what agents change is read again once it shows. */
  visible: boolean;
}) {
  const id = workspace.id;
  const indexState = useApp((s) => s.fileIndex[id]);
  const request = useApp((s) => s.filesRequest[id]);
  const turnsEnded = useWorkspaceSelector(session, (s) => s.turnsEnded);
  const worktreesChanged = useWorkspaceSelector(session, (s) => s.worktreesChanged);
  const [mode, setMode] = useState<"files" | "search">("files");
  const [selected, setSelected] = useState<string | null>(null);
  const [gotoLine, setGotoLine] = useState<(GotoLine & { path: string }) | null>(null);
  const [buffers, setBuffers] = useState<Record<string, Buffer>>({});
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [preview, setPreview] = useState(true);
  // An HTML file opens as source: rendering it runs its scripts.
  const [htmlPreview, setHtmlPreview] = useState(false);
  const filterInput = useRef<HTMLInputElement>(null);
  const searchInput = useRef<HTMLInputElement>(null);
  const buffersRef = useRef(buffers);
  buffersRef.current = buffers;

  const patchBuffer = useCallback((path: string, patch: Partial<Buffer>) => {
    setBuffers((all) => ({ ...all, [path]: { ...EMPTY_BUFFER, ...all[path], ...patch } }));
  }, []);

  // The list, on open and whenever agents may have added or removed files; while the view is
  // hidden, once it shows again.
  const listed = useRef<string | null>(null);
  useEffect(() => {
    if (!visible) return;
    const version = `${id}:${turnsEnded}:${worktreesChanged}`;
    if (listed.current === version) return;
    void loadFileIndex(id, listed.current === null ? 60_000 : 0);
    listed.current = version;
  }, [id, turnsEnded, worktreesChanged, visible]);

  const read = useCallback(
    async (path: string) => {
      const known = buffersRef.current[path];
      if (!known?.content) patchBuffer(path, { loading: true, error: null });
      try {
        const content = await client.call("files.read", { workspace: id, path });
        const current = buffersRef.current[path];
        if (current?.draft != null && current.content && current.content.version !== content.version) {
          // Unsaved edits: keep them and say the file moved on.
          patchBuffer(path, { diskChanged: true, loading: false });
          return;
        }
        if (current?.draft != null) {
          patchBuffer(path, { loading: false });
          return;
        }
        patchBuffer(path, { content, loading: false, error: null, diskChanged: false, conflict: false });
      } catch (error) {
        patchBuffer(path, { loading: false, error: errorMessage(error) });
      }
    },
    [id, patchBuffer],
  );

  const reveal = useCallback((path: string) => {
    setExpanded((current) => {
      const next = new Set(current);
      for (const dir of ancestors(path)) next.add(dir);
      return next;
    });
  }, []);

  const open = useCallback(
    (path: string, line: number | null = null) => {
      setSelected(path);
      reveal(path);
      setGotoLine(line ? { path, line, nonce: Date.now() } : null);
      if (!buffersRef.current[path]?.content) void read(path);
    },
    [read, reveal],
  );

  // Requests from elsewhere: a chat link, the diff, ⌘P, ⇧⌘F.
  const handled = useRef(0);
  useEffect(() => {
    if (!request || request.nonce === handled.current) return;
    handled.current = request.nonce;
    if (request.kind === "open") {
      setMode("files");
      open(request.path, request.line);
    } else if (request.kind === "goto") {
      setMode("files");
      requestAnimationFrame(() => filterInput.current?.select());
    } else {
      setMode("search");
      requestAnimationFrame(() => searchInput.current?.select());
    }
  }, [request, open]);

  // The open file follows what agents write, when it has no unsaved edits.
  const lastTurns = useRef(turnsEnded);
  useEffect(() => {
    if (!visible || lastTurns.current === turnsEnded) return;
    lastTurns.current = turnsEnded;
    if (selected) void read(selected);
  }, [turnsEnded, selected, read, visible]);

  // A daemon restarted serves previews under a new URL.
  const connected = useApp((s) => s.connection.status === "open");
  const wasConnected = useRef(connected);
  useEffect(() => {
    if (connected && !wasConnected.current && selected) void read(selected);
    wasConnected.current = connected;
  }, [connected, selected, read]);

  const save = useCallback(
    async (path: string, force = false) => {
      const buffer = buffersRef.current[path];
      if (!buffer?.content || buffer.draft == null || buffer.saving) return;
      patchBuffer(path, { saving: true });
      try {
        const content = await client.call("files.write", {
          workspace: id,
          path,
          text: buffer.draft,
          expected_version: force ? undefined : buffer.content.version,
        });
        const draftNow = buffersRef.current[path]?.draft;
        patchBuffer(path, {
          content,
          // Typed while saving: still unsaved.
          draft: draftNow != null && draftNow !== buffer.draft ? draftNow : null,
          saving: false,
          conflict: false,
          diskChanged: false,
        });
      } catch (error) {
        if (error instanceof RpcCallError && error.code === CONFLICT && !buffer.content.read_only) {
          patchBuffer(path, { saving: false, conflict: true });
        } else {
          patchBuffer(path, { saving: false });
          toast(`Could not save ${basename(path)}: ${errorMessage(error)}`, "error");
        }
      }
    },
    [id, patchBuffer],
  );

  const reload = useCallback(
    (path: string) => {
      patchBuffer(path, { draft: null, diskChanged: false, conflict: false });
      // After the draft is gone, so that the fresh content is taken.
      setTimeout(() => void read(path), 0);
    },
    [patchBuffer, read],
  );

  // Files named in a rendered markdown file open here too.
  const linker = useWorkspaceFileLinker(id, workspace.path);

  const dirty = useMemo(
    () => new Set(Object.entries(buffers).filter(([, b]) => b.draft != null).map(([path]) => path)),
    [buffers],
  );

  return (
    <FileLinkContext.Provider value={linker}>
    <div className="flex h-full min-h-0" data-testid="files-panel">
      <aside className="flex w-[300px] shrink-0 flex-col border-r border-line">
        <div className="flex h-9 shrink-0 items-center gap-1 border-b border-line px-2" role="tablist">
          <ModeButton active={mode === "files"} onClick={() => setMode("files")} shortcut={kbd("⌘P")}>
            <File size={12} /> Files
          </ModeButton>
          <ModeButton active={mode === "search"} onClick={() => setMode("search")} shortcut={kbd("⇧⌘F")}>
            <Search size={12} /> Search
          </ModeButton>
          {indexState?.loading && <Spinner size={11} className="ml-auto text-faint" />}
        </div>
        <div className={cx("min-h-0 flex-1 flex-col", mode === "files" ? "flex" : "hidden")}>
          <FileList
            workspace={workspace}
            inputRef={filterInput}
            selected={selected}
            dirty={dirty}
            expanded={expanded}
            onToggle={(dir) =>
              setExpanded((current) => {
                const next = new Set(current);
                if (next.has(dir)) next.delete(dir);
                else next.add(dir);
                return next;
              })
            }
            onOpen={(path) => open(path)}
          />
        </div>
        <div className={cx("min-h-0 flex-1 flex-col", mode === "search" ? "flex" : "hidden")}>
          <SearchPane workspaceId={id} inputRef={searchInput} onOpen={open} />
        </div>
      </aside>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {selected ? (
          <FileView
            workspaceId={id}
            path={selected}
            buffer={buffers[selected] ?? EMPTY_BUFFER}
            gotoLine={gotoLine?.path === selected ? gotoLine : null}
            preview={isHtml(selected) ? htmlPreview : preview}
            onPreview={isHtml(selected) ? setHtmlPreview : setPreview}
            onChange={(text) => {
              const content = buffersRef.current[selected]?.content;
              patchBuffer(selected, { draft: content?.text === text ? null : text });
            }}
            onSave={(force) => void save(selected, force)}
            onReload={() => reload(selected)}
            here={!workspace.moved_to}
          />
        ) : (
          <EmptyState icon={<FileCode size={22} />} title="No file open">
            Pick a file in the tree, find one with <Kbd>{kbd("⌘P")}</Kbd>, or search their content with{" "}
            <Kbd>{kbd("⇧⌘F")}</Kbd>.
          </EmptyState>
        )}
      </div>
    </div>
    </FileLinkContext.Provider>
  );
}

function ModeButton({
  active,
  onClick,
  shortcut,
  children,
}: {
  active: boolean;
  onClick: () => void;
  shortcut: string;
  children: React.ReactNode;
}) {
  return (
    <button
      role="tab"
      aria-selected={active}
      onClick={onClick}
      title={shortcut}
      className={cx(
        "flex h-6 items-center gap-1.5 rounded-md px-2 text-[12px]",
        active ? "bg-hover text-fg" : "text-muted hover:text-fg",
      )}
    >
      {children}
    </button>
  );
}

function FileList({
  workspace,
  inputRef,
  selected,
  dirty,
  expanded,
  onToggle,
  onOpen,
}: {
  workspace: WorkspaceInfo;
  inputRef: React.RefObject<HTMLInputElement | null>;
  selected: string | null;
  dirty: Set<string>;
  expanded: Set<string>;
  onToggle: (dir: string) => void;
  onOpen: (path: string) => void;
}) {
  const indexState = useApp((s) => s.fileIndex[workspace.id]);
  const index = indexState?.index ?? null;
  const [filter, setFilter] = useState("");
  const [highlight, setHighlight] = useState(0);
  const matches = useMemo(() => (index ? filterFiles(index.paths, filter, 200) : []), [index, filter]);
  useEffect(() => setHighlight(0), [filter]);
  const trees = useMemo(() => index?.roots.map(buildTree) ?? [], [index]);
  const list = useRef<HTMLDivElement>(null);
  const { onContextMenu, menu } = useFileMenu(useFileLinker());

  useEffect(() => {
    list.current?.querySelector('[data-highlighted="true"]')?.scrollIntoView({ block: "nearest" });
  }, [highlight]);

  return (
    <>
      <div className="shrink-0 border-b border-line p-2">
        <input
          ref={inputRef}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setHighlight((h) => Math.min(h + 1, Math.max(matches.length - 1, 0)));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setHighlight((h) => Math.max(h - 1, 0));
            } else if (e.key === "Enter" && matches[highlight]) {
              e.preventDefault();
              onOpen(matches[highlight]);
            } else if (e.key === "Escape" && filter) {
              e.preventDefault();
              setFilter("");
            }
          }}
          placeholder="Go to file…"
          className="h-7 w-full rounded-md border border-line bg-bg px-2 text-[12.5px] text-fg placeholder:text-faint focus:border-accent/60 focus:outline-none"
          data-testid="file-filter"
        />
      </div>
      <div
        ref={list}
        className="min-h-0 flex-1 overflow-y-auto py-1 text-[12.5px]"
        onContextMenu={(event) => {
          const row = (event.target as Element).closest<HTMLElement>("[data-file], [data-file-result]");
          const path = row?.dataset.file ?? row?.dataset.fileResult;
          if (path) onContextMenu({ path, line: null })(event);
        }}
      >
        {!index ? (
          indexState?.error ? (
            <div className="px-3 py-2 text-[12px] text-danger selectable">Could not list the files: {indexState.error}</div>
          ) : (
            <div className="flex items-center gap-2 px-3 py-2 text-[12px] text-faint">
              <Spinner size={11} /> Listing the files…
            </div>
          )
        ) : filter.trim() ? (
          matches.length === 0 ? (
            <div className="px-3 py-2 text-[12px] text-faint">No file matches “{filter}”.</div>
          ) : (
            matches.map((path, i) => (
              <button
                key={path}
                onClick={() => onOpen(path)}
                onMouseEnter={() => setHighlight(i)}
                data-highlighted={i === highlight}
                className={cx(
                  "flex w-full min-w-0 items-baseline gap-2 px-3 py-[3px] text-left",
                  i === highlight ? "bg-hover" : "hover:bg-hover/60",
                  path === selected && "text-fg",
                )}
                data-file-result={path}
              >
                <span className="shrink-0 text-fg">{basename(path)}</span>
                <span className="min-w-0 truncate text-[11px] text-faint" dir="rtl">
                  {/* Right-to-left so that a long folder is cut at its start. */}
                  <bdi>{path.slice(0, path.length - basename(path).length - 1)}</bdi>
                </span>
              </button>
            ))
          )
        ) : (
          trees.map((tree, i) => (
            <TreeNode
              key={tree.path}
              dir={tree}
              label={index.roots[i].repo ? tree.name : "Notes"}
              depth={0}
              root
              expanded={expanded}
              selected={selected}
              dirty={dirty}
              onToggle={onToggle}
              onOpen={onOpen}
              truncated={index.roots[i].truncated}
            />
          ))
        )}
      </div>
      {menu}
    </>
  );
}

const TreeNode = memo(function TreeNode({
  dir,
  label,
  depth,
  root = false,
  truncated = false,
  expanded,
  selected,
  dirty,
  onToggle,
  onOpen,
}: {
  dir: TreeDir;
  label: string;
  depth: number;
  root?: boolean;
  truncated?: boolean;
  expanded: Set<string>;
  selected: string | null;
  dirty: Set<string>;
  onToggle: (dir: string) => void;
  onOpen: (path: string) => void;
}) {
  // Roots start open; folders start closed.
  const isOpen = root ? !expanded.has(`!${dir.path}`) : expanded.has(dir.path);
  const pad = 8 + depth * 12;
  return (
    <div>
      <button
        onClick={() => onToggle(root ? `!${dir.path}` : dir.path)}
        className={cx(
          "flex w-full min-w-0 items-center gap-1 py-[3px] pr-2 text-left hover:bg-hover/60",
          root ? "mt-1 font-medium text-fg" : "text-muted",
        )}
        style={{ paddingLeft: pad }}
        data-dir={dir.path}
      >
        {isOpen ? <ChevronDown size={12} className="shrink-0 text-faint" /> : <ChevronRight size={12} className="shrink-0 text-faint" />}
        {!root && (isOpen ? <FolderOpen size={12} className="shrink-0 text-faint" /> : <Folder size={12} className="shrink-0 text-faint" />)}
        <span className="truncate">{label}</span>
      </button>
      {isOpen && (
        <>
          {dir.dirs.map((child) => (
            <TreeNode
              key={child.path}
              dir={child}
              label={child.name}
              depth={depth + 1}
              expanded={expanded}
              selected={selected}
              dirty={dirty}
              onToggle={onToggle}
              onOpen={onOpen}
            />
          ))}
          {dir.files.map((path) => (
            <button
              key={path}
              onClick={() => onOpen(path)}
              className={cx(
                "flex w-full min-w-0 items-center gap-1.5 py-[3px] pr-2 text-left",
                path === selected ? "bg-hover text-fg" : "text-muted hover:bg-hover/60 hover:text-fg",
              )}
              style={{ paddingLeft: pad + 26 }}
              data-file={path}
            >
              <span className="truncate">{basename(path)}</span>
              {dirty.has(path) && <span title="Unsaved" className="h-1.5 w-1.5 shrink-0 rounded-full bg-attention" />}
            </button>
          ))}
          {root && dir.dirs.length === 0 && dir.files.length === 0 && (
            <div className="py-[3px] text-[12px] text-faint" style={{ paddingLeft: pad + 26 }}>
              Empty
            </div>
          )}
          {truncated && (
            <div className="py-[3px] text-[11.5px] text-attention" style={{ paddingLeft: pad + 26 }}>
              Too many files: use Go to file or Search.
            </div>
          )}
        </>
      )}
    </div>
  );
});

function SearchPane({
  workspaceId,
  inputRef,
  onOpen,
}: {
  workspaceId: string;
  inputRef: React.RefObject<HTMLInputElement | null>;
  onOpen: (path: string, line: number | null) => void;
}) {
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [regex, setRegex] = useState(false);
  const [result, setResult] = useState<FilesSearchResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [searching, setSearching] = useState(false);
  const generation = useRef(0);

  useEffect(() => {
    const q = query;
    if (!q.trim()) {
      setResult(null);
      setError(null);
      return;
    }
    const current = ++generation.current;
    const timer = setTimeout(async () => {
      setSearching(true);
      try {
        const found = await client.call("files.search", {
          workspace: workspaceId,
          query: q,
          regex,
          case_sensitive: caseSensitive,
        });
        if (current !== generation.current) return;
        setResult(found);
        setError(null);
      } catch (err) {
        if (current !== generation.current) return;
        setResult(null);
        setError(errorMessage(err));
      } finally {
        if (current === generation.current) setSearching(false);
      }
    }, 250);
    return () => clearTimeout(timer);
  }, [query, regex, caseSensitive, workspaceId]);

  const groups = useMemo(() => {
    const byPath = new Map<string, SearchMatch[]>();
    for (const match of result?.matches ?? []) {
      const list = byPath.get(match.path);
      if (list) list.push(match);
      else byPath.set(match.path, [match]);
    }
    return [...byPath.entries()];
  }, [result]);

  const pattern = useMemo(() => highlightPattern(query, regex, caseSensitive), [query, regex, caseSensitive]);

  return (
    <>
      <div className="shrink-0 space-y-1.5 border-b border-line p-2">
        <div className="flex items-center gap-1 rounded-md border border-line bg-bg pr-1 focus-within:border-accent/60">
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search in files…"
            className="h-7 min-w-0 flex-1 bg-transparent px-2 text-[12.5px] text-fg placeholder:text-faint focus:outline-none"
            data-testid="files-search"
          />
          <IconButton
            title="Match case"
            onClick={() => setCaseSensitive((v) => !v)}
            className={caseSensitive ? "bg-accent/20 text-accent" : undefined}
          >
            <CaseSensitive size={14} />
          </IconButton>
          <IconButton
            title="Regular expression"
            onClick={() => setRegex((v) => !v)}
            className={regex ? "bg-accent/20 text-accent" : undefined}
          >
            <Regex size={13} />
          </IconButton>
        </div>
        {(result || searching || error) && (
          <div className="flex items-center gap-1.5 text-[11px] text-faint">
            {searching && <Spinner size={10} />}
            {error ? (
              <span className="text-danger selectable">{error}</span>
            ) : result ? (
              <span>
                {result.matches.length}
                {result.truncated ? "+" : ""} result{result.matches.length === 1 ? "" : "s"} in {groups.length} file
                {groups.length === 1 ? "" : "s"}
              </span>
            ) : null}
          </div>
        )}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto py-1 text-[12px]" data-testid="search-results">
        {result && result.matches.length === 0 && <div className="px-3 py-2 text-faint">No result.</div>}
        {groups.map(([path, matches]) => (
          <SearchGroup key={path} path={path} matches={matches} pattern={pattern} onOpen={onOpen} />
        ))}
        {result?.truncated && (
          <div className="px-3 py-2 text-[11.5px] text-attention">More results than shown: refine the search.</div>
        )}
      </div>
    </>
  );
}

function SearchGroup({
  path,
  matches,
  pattern,
  onOpen,
}: {
  path: string;
  matches: SearchMatch[];
  pattern: RegExp | null;
  onOpen: (path: string, line: number | null) => void;
}) {
  const [open, setOpen] = useState(true);
  return (
    <div>
      <button
        onClick={() => setOpen((v) => !v)}
        className="flex w-full min-w-0 items-center gap-1 px-2 py-[3px] text-left hover:bg-hover/60"
        title={path}
      >
        {open ? <ChevronDown size={12} className="shrink-0 text-faint" /> : <ChevronRight size={12} className="shrink-0 text-faint" />}
        <span className="shrink-0 font-medium text-fg">{basename(path)}</span>
        <span className="min-w-0 truncate text-[11px] text-faint">{path.slice(0, path.length - basename(path).length - 1)}</span>
        <span className="ml-auto shrink-0 rounded bg-raised px-1 text-[10.5px] text-muted">{matches.length}</span>
      </button>
      {open &&
        matches.map((match) => (
          <button
            key={`${match.line}:${match.column}`}
            onClick={() => onOpen(path, match.line)}
            className="flex w-full min-w-0 items-baseline gap-2 py-[2px] pl-7 pr-2 text-left hover:bg-hover/60"
            data-match={`${path}:${match.line}`}
          >
            <span className="w-8 shrink-0 text-right font-mono text-[10.5px] text-faint">{match.line}</span>
            <span className="min-w-0 truncate font-mono text-[11.5px] text-muted">
              <Highlighted text={snippet(match.text, pattern)} pattern={pattern} />
            </span>
          </button>
        ))}
    </div>
  );
}

/** The line, cut before the match when it starts far in, so that the match shows. */
function snippet(line: string, pattern: RegExp | null): string {
  const text = line.trimStart();
  if (!pattern) return text;
  pattern.lastIndex = 0;
  const found = pattern.exec(text);
  if (!found || found.index < 30) return text;
  return `…${text.slice(found.index - 16)}`;
}

function highlightPattern(query: string, regex: boolean, caseSensitive: boolean): RegExp | null {
  if (!query) return null;
  try {
    const source = regex ? query : query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(source, caseSensitive ? "g" : "gi");
  } catch {
    return null;
  }
}

function Highlighted({ text, pattern }: { text: string; pattern: RegExp | null }) {
  if (!pattern) return <>{text}</>;
  const parts: React.ReactNode[] = [];
  let last = 0;
  pattern.lastIndex = 0;
  for (let match = pattern.exec(text); match && parts.length < 40; match = pattern.exec(text)) {
    if (match[0] === "") {
      pattern.lastIndex++;
      continue;
    }
    parts.push(text.slice(last, match.index));
    parts.push(
      <mark key={match.index} className="rounded-[2px] bg-attention/30 text-fg">
        {match[0]}
      </mark>,
    );
    last = match.index + match[0].length;
  }
  parts.push(text.slice(last));
  return <>{parts}</>;
}

function FileView({
  workspaceId,
  path,
  buffer,
  gotoLine,
  preview,
  onPreview,
  onChange,
  onSave,
  onReload,
  here,
}: {
  workspaceId: string;
  path: string;
  /** Its files are on this machine: not those of a workspace that runs on another. */
  here: boolean;
  buffer: Buffer;
  gotoLine: GotoLine | null;
  preview: boolean;
  onPreview: (preview: boolean) => void;
  onChange: (text: string) => void;
  onSave: (force?: boolean) => void;
  onReload: () => void;
}) {
  const content = buffer.content;
  const markdown = /\.(md|mdx|markdown)$/i.test(path);
  const previewUrl = content?.kind === "text" || content?.kind === "too_large" ? content.preview_url : null;
  // A page too large for the editor has no source to show: only its rendering.
  const tooLargePage = content?.kind === "too_large" && previewUrl != null;
  const renderable = (markdown && content?.kind === "text") || (previewUrl != null && !tooLargePage);
  const showPreview = renderable && preview && buffer.draft == null && !gotoLine;
  const readOnly = content?.read_only ?? true;
  const parts = path.split("/");
  const openable = content?.kind === "media";
  const commentable = markdown && content?.kind === "text";
  const fileShare = useFileShare(workspaceId, commentable && here ? path : null);
  const [sharing, setSharing] = useState(false);
  const fileComments = useFileComments(workspaceId, commentable ? path : null, `${content?.version}:${fileShare?.comments_rev ?? 0}`);
  const openThreads = fileComments.comments.filter((c) => c.reply_to == null && !c.resolved).length;
  // Shown when the file has open comments, until the user says otherwise for this file.
  const [commentsShown, setCommentsShown] = useState<boolean | null>(null);
  useEffect(() => setCommentsShown(null), [path]);
  const commentsOpen = commentsShown ?? openThreads > 0;

  return (
    <>
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-line px-3 text-[12px]">
        <span className="min-w-0 truncate font-mono text-[11.5px] text-muted selectable" title={path}>
          {parts.slice(0, -1).map((part, i) => (
            <span key={i}>
              <span className={i === 0 ? "text-fg/80" : undefined}>{part === ".chauffeur" ? "Notes" : part}</span>
              <span className="px-0.5 text-faint">/</span>
            </span>
          ))}
          <span className="text-fg">{parts[parts.length - 1]}</span>
        </span>
        {buffer.draft != null && <span className="shrink-0 text-[11px] text-attention">Unsaved</span>}
        <div className="ml-auto flex shrink-0 items-center gap-1.5">
          {content && readOnly && (
            <span title="This workspace's files cannot be edited here" className="inline-flex items-center gap-1 text-[11.5px] text-faint">
              <Lock size={11} /> Read-only
            </span>
          )}
          {content && here && (
            <Button size="sm" variant="ghost" onClick={() => void revealFile(workspaceId, path)} title="Show the file where it is on disk" data-testid="reveal-file">
              <FolderSearch size={12} /> {REVEAL_LABEL}
            </Button>
          )}
          {openable && content && here && (
            <Button size="sm" variant="ghost" onClick={() => void openInDefaultApp(workspaceId, path)} title="Open it in the system's app for this kind of file">
              <ExternalLink size={12} /> Open in default app
            </Button>
          )}
          {commentable && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                onPreview(true);
                setCommentsShown(!(showPreview && commentsOpen));
              }}
              title="Comment on passages of this file and send the comments to a chat"
              data-testid="file-comments-toggle"
            >
              <MessageSquare size={12} /> Comments{openThreads > 0 ? ` ${openThreads}` : ""}
            </Button>
          )}
          {commentable && here && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => setSharing(true)}
              title="Share this file alone with colleagues, live, for them to comment on it"
              data-testid="file-share"
            >
              <Share2 size={12} /> {fileShare?.share && !fileShare.share.stopped ? "Shared" : "Share"}
            </Button>
          )}
          {renderable && (
            <Button size="sm" variant="ghost" onClick={() => onPreview(!preview)} title="Rendered or source" data-testid="preview-toggle">
              {preview ? <FileCode size={12} /> : <Eye size={12} />} {preview ? "Source" : "Preview"}
            </Button>
          )}
          {buffer.draft != null && (
            <>
              <Button size="sm" variant="ghost" onClick={onReload} title="Discard your edits">
                <RotateCcw size={12} /> Revert
              </Button>
              <Button size="sm" variant="primary" onClick={() => onSave()} busy={buffer.saving} data-testid="save-file">
                <Save size={12} /> Save <Kbd>{kbd("⌘S")}</Kbd>
              </Button>
            </>
          )}
        </div>
      </div>
      {sharing && <ShareDialog target={{ kind: "file", workspace: workspaceId, path }} state={fileShare} onClose={() => setSharing(false)} />}
      {(buffer.conflict || buffer.diskChanged) && (
        <div className="flex shrink-0 items-center gap-2 border-b border-attention/30 bg-attention/10 px-3 py-1.5 text-[12px] text-attention">
          <AlertTriangle size={13} className="shrink-0" />
          <span className="min-w-0 flex-1">
            {buffer.conflict
              ? "The file changed on disk since you opened it (an agent?): your save was not written."
              : "The file changed on disk; you have unsaved edits."}
          </span>
          <Button size="sm" onClick={onReload}>
            Reload theirs
          </Button>
          <Button size="sm" variant="danger" onClick={() => onSave(true)}>
            Overwrite with mine
          </Button>
        </div>
      )}
      <div className="relative min-h-0 flex-1">
        {buffer.error ? (
          <EmptyState icon={<AlertTriangle size={22} />} title="Could not open the file">
            {buffer.error}
          </EmptyState>
        ) : !content ? (
          <div className="flex h-full items-center justify-center text-faint">
            <Spinner />
          </div>
        ) : content.kind === "image" ? (
          <div className="flex h-full items-center justify-center overflow-auto p-6">
            <img
              src={`data:${content.mime};base64,${content.base64}`}
              alt={path}
              className="max-h-full max-w-full rounded border border-line"
            />
          </div>
        ) : content.kind === "media" ? (
          <MediaPlayer workspaceId={workspaceId} path={path} mime={content.mime ?? ""} />
        ) : content.kind === "binary" ? (
          <EmptyState icon={<File size={22} />} title="Binary file">
            {formatSize(content.size)}: not shown.
          </EmptyState>
        ) : (showPreview || tooLargePage) && previewUrl ? (
          // An opaque origin (no allow-same-origin), as a design chat's screens: nothing of the app reaches the page.
          <iframe
            key={`${previewUrl}#${content.version}`}
            src={previewUrl}
            title={basename(path)}
            sandbox="allow-scripts allow-forms"
            referrerPolicy="no-referrer"
            className="h-full w-full border-0 bg-white"
            data-testid="html-preview"
          />
        ) : content.kind === "too_large" ? (
          <EmptyState icon={<File size={22} />} title="File too large">
            {formatSize(content.size)}: open it in your editor.
          </EmptyState>
        ) : showPreview ? (
          <CommentedMarkdown
            workspaceId={workspaceId}
            path={path}
            text={content.text ?? ""}
            state={fileComments}
            panel={commentsOpen}
            onPanel={setCommentsShown}
          />
        ) : (
          <Suspense
            fallback={
              <div className="flex h-full items-center justify-center text-faint">
                <Spinner />
              </div>
            }
          >
            <CodeEditor
              docKey={path}
              value={buffer.draft ?? content.text ?? ""}
              path={path}
              readOnly={readOnly}
              gotoLine={gotoLine}
              onChange={onChange}
              onSave={() => onSave()}
            />
          </Suspense>
        )}
      </div>
    </>
  );
}

function isHtml(path: string): boolean {
  return /\.html?$/i.test(path);
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1 << 20) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1 << 20)).toFixed(1)} MB`;
}

/** A video, a sound or a PDF of the workspace, streamed by the daemon (`/media`). */
function MediaPlayer({ workspaceId, path, mime }: { workspaceId: string; path: string; mime: string }) {
  // Again on reconnection: the daemon's port may change.
  const connection = useApp((s) => s.connection.status);
  const src = useMemo(() => {
    const base = client.httpUrl(`/media/${encodeURIComponent(workspaceId)}`);
    return base && `${base}&path=${encodeURIComponent(path)}`;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaceId, path, connection]);
  const [failed, setFailed] = useState(false);
  useEffect(() => setFailed(false), [src]);
  if (!src || failed) {
    return (
      <EmptyState icon={<File size={22} />} title="Cannot play this file here">
        Open it in its default app instead.
      </EmptyState>
    );
  }
  if (mime === "application/pdf") {
    // WebKit's own PDF viewer (zoom, search, selection); a `sandbox` attribute would block it.
    return <iframe key={src} src={src} title={basename(path)} className="h-full w-full border-0 bg-white" />;
  }
  return (
    <div className="flex h-full items-center justify-center overflow-auto p-6">
      {mime.startsWith("audio/") ? (
        <audio key={src} src={src} controls className="w-full max-w-[560px]" onError={() => setFailed(true)} />
      ) : (
        <video
          key={src}
          src={src}
          controls
          playsInline
          className="max-h-full max-w-full rounded border border-line bg-black"
          onError={() => setFailed(true)}
        />
      )}
    </div>
  );
}

async function openInDefaultApp(workspaceId: string, path: string) {
  try {
    await client.call("files.open", { workspace: workspaceId, path });
  } catch (error) {
    toast(`Could not open ${basename(path)}: ${errorMessage(error)}`, "error");
  }
}
