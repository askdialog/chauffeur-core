import type { LinearIssue, LinearMilestone, PermissionMode, TicketRef, WorkspaceSharePreview } from "@chauffeur/protocol";
import {
  AlertTriangle,
  Diamond,
  Download,
  Folder,
  GitBranch,
  Info,
  Layers,
  Link2,
  Loader2,
  PenTool,
  Plus,
  Search,
  Telescope,
  Ticket,
  Type,
  User,
  X,
} from "lucide-react";
import { lazy, type ReactNode, Suspense, useEffect, useRef, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import { relativeTime, workspaceName, workspaceRepoNames } from "@/lib/format";
import type { DesignOption } from "@/lib/designs";
import {
  type CreateDraft,
  client,
  loadRepos,
  leaveOfficeFor,
  openCreate,
  openOfficeChat,
  rememberCreation,
  selectChat,
  selectWorkspace,
  setTab,
  takeCreateRetry,
  toast,
  upsertChats,
  upsertWorkspace,
  useApp,
} from "@/state/app";
import { designChoice, effectiveLoadout, initialChoice, type ModelChoice } from "@/lib/models";
import { toImageInputs } from "@/lib/images";
import {
  branchErrorOf,
  branchPrefix,
  branchPreview,
  checkoutBranch,
  type CreateKind,
  isDesignShareLink,
  isClaudeArtifactLink,
  isWorkspaceShareLink,
  KIND_HINTS,
  type LinkedTicket,
  startsTicket,
  toggledRepo,
  withoutTicketRepos,
  withTicketRepos,
} from "@/lib/newWorkspace";
import { AttachButton, DropOverlay, ImageStrip, useImageAttachments } from "./chat/ImageAttachments";
import { useSlashMenu } from "./chat/useSlashMenu";
import { ModelPicker } from "./ModelPicker";
import { DEFAULT_PERMISSION_MODE } from "@/lib/permissions";
import { PermissionPicker } from "./PermissionPicker";
import { StateDot, useTicketSearch } from "./TicketSearch";
import { GithubSetup } from "./GithubSetup";
import { ClaudeArtifactImport } from "./ClaudeArtifactImport";
import { RepoPopover } from "./RepoPicker";
import { Button, cx, DesignOnlyMode, Kbd, Modal, Popover, ReadOnlyMode } from "./ui";
import { signIn, useIdentity } from "@/state/identity";
import { kbd } from "../lib/keys";

// The Office's, loaded with it: offered only from the 3D view.
const DesignPicker = lazy(() => import("./DesignPicker").then((m) => ({ default: m.DesignPicker })));

export function CreateWorkspaceDialog() {
  const open = useApp((s) => s.createOpen);
  const link = useApp((s) => s.createWorkspaceShare);
  // Remount on each opening: a fresh form every time.
  return open ? <CreateForm key={link} onClose={() => openCreate(false)} /> : null;
}

/** Why the last attempt failed: a creation refused (the draft is kept), or a check of the form. */
type FormError = { message: string; created: boolean; branch: { suggestion: string | null } | null };

/** The settings the form previews with: the branch prefix and the move to In Progress. */
type FormSettings = { prefix: string; startTickets: boolean };

function CreateForm({ onClose }: { onClose: () => void }) {
  const signingIn = useIdentity((s) => s.signingIn);
  const signInError = useIdentity((s) => s.error);
  const signedInEmail = useApp((s) => s.identity?.email ?? null);
  const [settings, setSettings] = useState<FormSettings | null>(null);
  useEffect(() => {
    let live = true;
    client.call("settings.get", {}).then(
      (s) => { if (live) setSettings({ prefix: branchPrefix(s.workspaces), startTickets: s.linear.enabled && s.linear.start_tickets }); },
      () => { if (live) setSettings({ prefix: branchPrefix(null), startTickets: false }); },
    );
    return () => { live = false; };
  }, []);
  const repos = useApp((s) => s.repos);
  const reposError = useApp((s) => s.reposError);
  // An exploration: the repos at their base branch, read-only chats. Chosen when opening.
  // A creation that failed after the dialog closed comes back filled in, with its error.
  const [retry] = useState(takeCreateRetry);
  const [designChat, setDesignChat] = useState(retry?.draft.designChat ?? false);
  const [explore, setExplore] = useState(() => retry?.draft.explore ?? useApp.getState().createExplore);
  const kind: CreateKind = designChat ? "design" : explore ? "explore" : "workspace";
  // A child workspace of this one: a workspace, never an exploration, on the parent's repos at first.
  const [parentId] = useState(() => retry?.draft.parent ?? useApp.getState().createParent);
  const parent = useApp((s) => (parentId ? s.workspaces[parentId] : undefined));
  const [selected, setSelected] = useState<string[] | null>(() => {
    const inherited = parent ? workspaceRepoNames(parent) : [];
    return retry?.draft.repos ?? (inherited.length > 0 ? inherited : null);
  });
  const [repoSources, setRepoSources] = useState<Record<string, string>>(retry?.draft.repoSources ?? {});
  const [tickets, setTickets] = useState<TicketRef[]>(retry?.draft.tickets ?? []);
  const [linked, setLinked] = useState<Record<string, LinkedTicket>>(retry?.draft.linked ?? {});
  const [workspaceShare, setWorkspaceShare] = useState(retry?.draft.workspaceShare ?? useApp.getState().createWorkspaceShare);
  const [importing, setImporting] = useState(() => !!workspaceShare.trim());
  const [designShare, setDesignShare] = useState(retry?.draft.designShare ?? "");
  const [importingDesign, setImportingDesign] = useState(() => !!designShare.trim());
  const [name, setName] = useState(retry?.draft.name ?? "");
  const [branch, setBranch] = useState(retry?.draft.branch ?? "");
  const [editNames, setEditNames] = useState(() => !!(retry?.draft.name || retry?.draft.branch));
  const [milestone, setMilestone] = useState(
    retry?.draft.milestone ? { id: retry.draft.milestone, name: retry.draft.milestoneName ?? retry.draft.milestone } : null,
  );
  const models = useApp((s) => s.models);
  const loadout = useApp((s) => s.loadout);
  // The first loadout entry until the user picks something else.
  const [picked, setPicked] = useState<ModelChoice | null>(retry?.draft.choice ?? null);
  const favorites = effectiveLoadout(loadout, models);
  // A design starts on the first Claude favorite until the user picks another model, Codex's included.
  const choice = picked ?? (designChat ? designChoice(favorites) : initialChoice(favorites));
  const [mode, setMode] = useState<PermissionMode>(retry?.draft.mode ?? DEFAULT_PERMISSION_MODE);
  const [prompt, setPrompt] = useState(retry?.draft.prompt ?? "");
  // From the 3D view, a design can come with the work (docs/office.md).
  const [office] = useState(() => useApp.getState().officeShown);
  const [design, setDesign] = useState<DesignOption | null>(retry?.draft.design ?? null);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<FormError | null>(
    retry ? { message: retry.error, created: true, branch: null } : null,
  );
  const [popover, setPopover] = useState<"repos" | "tickets" | null>(null);
  const repoButton = useRef<HTMLButtonElement>(null);
  const ticketButton = useRef<HTMLButtonElement>(null);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const attachments = useImageAttachments(() => promptRef.current?.focus());
  const prefill = useApp((s) => s.createPrefill);
  const preview = useSharePreview(importing && kind !== "design" ? workspaceShare : "", !!signedInEmail);

  useEffect(() => {
    // Read and consume together: StrictMode may run this effect twice before the next render.
    const incoming = useApp.getState().createPrefill;
    if (!incoming) return;
    useApp.setState({ createPrefill: "" });
    setPrompt((current) => current ? `${current}\n\n${incoming}` : incoming);
    promptRef.current?.focus();
  }, [prefill]);

  useEffect(() => {
    if (repos == null) void loadRepos();
  }, [repos]);

  // Defaults preselected once the repo list is known; none when it cannot be listed.
  useEffect(() => {
    if (selected == null && repos) setSelected(repos.filter((r) => r.is_default).map((r) => r.key));
    else if (selected == null && reposError) setSelected([]);
  }, [repos, reposError, selected]);
  const selection = selected ?? [];
  const choiceOfRepos = { selection, sources: repoSources };
  const setChoiceOfRepos = (next: { selection: string[]; sources: Record<string, string> }) => {
    setSelected(next.selection);
    setRepoSources(next.sources);
  };
  // `/` lists the skills of the chosen repos' clones, as the first chat will see them.
  const slash = useSlashMenu(prompt, { repos: selection, agent: choice.agent }, (command) => {
    setPrompt(`/${command.name} `);
    promptRef.current?.focus();
  });

  const toggleRepo = (repo: string) => setChoiceOfRepos(toggledRepo(choiceOfRepos, repo));

  const addTicket = (ticket: TicketRef, ticketRepos: string[], issue: LinearIssue | null) => {
    setTickets((current) =>
      current.some((t) => t.identifier === ticket.identifier) ? current : [...current, ticket],
    );
    if (issue) {
      setLinked((current) => ({
        ...current,
        [issue.identifier]: {
          identifier: issue.identifier,
          branch_name: issue.branch_name,
          state: issue.state,
          state_type: issue.state_type,
          repos: issue.repos,
        },
      }));
    }
    if (ticketRepos.length > 0) setChoiceOfRepos(withTicketRepos(choiceOfRepos, ticket.identifier, ticketRepos));
  };
  const removeTicket = (identifier: string) => {
    setTickets((current) => current.filter((t) => t.identifier !== identifier));
    setChoiceOfRepos(withoutTicketRepos(choiceOfRepos, identifier));
  };

  const shareLink = importing && kind !== "design" ? workspaceShare.trim() : "";
  const designLink = importingDesign && kind === "design" ? designShare.trim() : "";
  const noRepo = selected != null && selection.length === 0;
  const claudeImport = isClaudeArtifactLink(designLink);
  const [claudeReadyLink, setClaudeReadyLink] = useState("");

  const submit = async () => {
    if (creating) return;
    if (attachments.reading > 0) {
      setError({ message: "The images are still loading.", created: false, branch: null });
      return;
    }
    if (selected == null || (noRepo && !designChat)) return;
    if (claudeImport && claudeReadyLink !== designLink) return;
    if (designLink && !isDesignShareLink(designLink) && !claudeImport) {
      setError({ message: "Paste a Chauffeur design or public Claude artifact link, or close the import.", created: false, branch: null });
      return;
    }
    setCreating(true);
    setError(null);
    const draft: CreateDraft = {
      designChat,
      workspaceShare: shareLink,
      explore,
      repos: selection,
      tickets,
      name,
      branch,
      milestone: milestone?.id ?? "",
      choice,
      mode,
      prompt,
      parent: parentId,
      design,
      linked,
      repoSources,
      milestoneName: milestone?.name,
      designShare: designLink,
    };
    try {
      // Returns at once, in `creating` state, with the first chat: the dialog closes and the
      // conversation shows the setup going on; the agent starts once the workspace is ready.
      const result = await client.call("workspace.create", {
        repos: selection,
        no_repos: noRepo,
        workspace_share: shareLink || undefined,
        tickets: tickets.map((t) => t.identifier),
        milestone: milestone?.id,
        name: name.trim() || undefined,
        branch: explore || designChat ? undefined : branch.trim() || undefined,
        chat_kind: designChat ? "design" : undefined,
        prompt: prompt.trim() || undefined,
        images: toImageInputs(attachments.images),
        agent: choice.agent,
        model: choice.model ?? undefined,
        effort: choice.effort ?? undefined,
        fast: choice.fast || undefined,
        // An exploration's chats are read-only whatever is asked.
        permission_mode: explore || designChat ? undefined : mode,
        background: true,
        explore: explore || designChat,
        parent: parentId ?? undefined,
        design: designChat ? (designLink ? { kind: claudeImport ? "claude" : "share", url: designLink } : undefined) : design?.design,
      });
      const id = result.workspace.id;
      upsertWorkspace(result.workspace);
      if (result.chat) {
        upsertChats([result.chat]);
        selectChat(id, result.chat.id);
      }
      // A creation that fails later reopens this form, filled in, with the error.
      rememberCreation(id, draft);
      if (useApp.getState().officeShown && !designChat) {
        // From the Office, the user stays there: the workspace's island appears with its first
        // robot, its chat beside the scene. Without a first prompt, that chat waits for one.
        // The workspace exists already: a failure here must not let the form create it again.
        const chat =
          result.chat ??
          (await client
            .call("chat.create", {
              workspace: id,
              agent: choice.agent,
              model: choice.model ?? undefined,
              effort: choice.effort ?? undefined,
              fast: choice.fast || undefined,
            })
            .catch((error: unknown) => {
              toast(`Could not start a chat in ${result.workspace.slug}: ${errorMessage(error)}`, "error");
              return null;
            }));
        if (chat) {
          upsertChats([chat]);
          openOfficeChat({ workspace: id, chat: chat.id });
        }
      } else {
        if (designChat) leaveOfficeFor(id);
        else selectWorkspace(id);
        setTab(id, "chat");
      }
      onClose();
    } catch (err) {
      const branchError = branchErrorOf(err);
      if (branchError) setEditNames(true);
      setError({ message: errorMessage(err), created: true, branch: branchError });
      setCreating(false);
    }
  };

  const branchToBe = branchPreview({
    kind,
    prefix: settings?.prefix ?? "…",
    branch,
    name,
    child: !!parentId,
    tickets,
    linked,
    shareTitle: preview.state === "ready" ? preview.preview.title : null,
  });
  const checkout = checkoutBranch(repos, selection);
  const starting = settings
    ? tickets.filter((t) => startsTicket(linked[t.identifier], settings.startTickets, kind)).map((t) => t.identifier)
    : [];
  const what = designChat ? "design" : explore ? "exploration" : parentId ? "sub-workspace" : "workspace";
  const label = error?.created
    ? "Try again"
    : shareLink
      ? "Import and create"
      : designChat
        ? "Create design"
        : explore
          ? "Start exploring"
          : parentId
            ? "Create sub-workspace"
            : "Create workspace";

  return (
    <Modal
      open
      onClose={onClose}
      title={
        parent ? (
          `New sub-workspace of ${workspaceName(parent)}`
        ) : (
          <KindSwitch
            kind={kind}
            onChange={(next) => {
              setDesignChat(next === "design");
              setExplore(next === "explore");
            }}
          />
        )
      }
      width={680}
      footer={
        <>
          <div className="mr-auto flex min-w-0 items-center">
            {parentId ? null : shareLink ? (
              signedInEmail && (
                <span className="flex items-center gap-1.5 pl-1.5 text-[11.5px] text-faint">
                  <User size={12} /> Signed in as {signedInEmail}
                </span>
              )
            ) : designChat ? (
              !importingDesign && (
                <Button variant="ghost" size="sm" onClick={() => setImportingDesign(true)} data-testid="import-design-button">
                  <Download size={12} /> Import a design…
                </Button>
              )
            ) : (
              !importing && (
                <Button variant="ghost" size="sm" onClick={() => setImporting(true)} data-testid="import-workspace-button">
                  <Download size={12} /> Import a shared workspace…
                </Button>
              )
            )}
          </div>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={submit}
            busy={creating}
            disabled={selected == null || (noRepo && !designChat) || (claudeImport && claudeReadyLink !== designLink)}
            title={selected == null ? "Listing the repos…" : noRepo && !designChat ? "Pick at least one repo" : undefined}
            data-testid="create-workspace"
          >
            {label} <Kbd>{kbd("⌘↵")}</Kbd>
          </Button>
        </>
      }
    >
      <div
        className="space-y-3"
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            void submit();
          }
        }}
      >
        {!parentId && <div className="-mt-1.5 pl-0.5 text-[11.5px] text-faint" data-testid="kind-hint">{KIND_HINTS[kind]}</div>}

        {error && (
          <div className="flex items-start gap-[9px] rounded-[7px] border border-danger/30 bg-danger/10 px-[11px] py-[9px] text-[12.5px] text-danger selectable" data-testid="create-error">
            <AlertTriangle size={14} className="mt-0.5 shrink-0" />
            <div className="min-w-0">
              {error.created && <div className="font-medium text-fg">The {what} could not be created. Nothing was left behind.</div>}
              <span>{error.message}</span>
              {error.created && <span> Your draft is kept below.</span>}
            </div>
          </div>
        )}

        {!parentId && <GithubSetup compact />}

        <fieldset disabled={creating} className="min-w-0 space-y-3 disabled:opacity-60">
          {shareLink !== "" || (importing && kind !== "design") ? (
            <ImportCard
              link={workspaceShare}
              onLink={setWorkspaceShare}
              placeholder="https://share.example.com/w/…"
              onClose={() => {
                setImporting(false);
                setWorkspaceShare("");
              }}
              testId="import-workspace-link"
            >
              {!workspaceShare.trim() ? (
                <span className="text-faint">Paste the link a colleague shared: its conversations and notes are copied into new chats.</span>
              ) : !isWorkspaceShareLink(workspaceShare) ? (
                <span className="text-faint">Paste a workspace share link (…/w/&lt;id&gt;).</span>
              ) : !signedInEmail ? (
                <div className="flex items-center gap-2">
                  <Button size="sm" onClick={() => void signIn()} disabled={signingIn}>
                    {signingIn ? "Finish in your browser…" : "Sign in with Google to import"}
                  </Button>
                  {signInError && <span className="text-danger">{signInError}</span>}
                </div>
              ) : preview.state === "loading" ? (
                <span className="flex items-center gap-1.5 text-faint">
                  <Loader2 size={12} className="animate-spin" /> Reading the share…
                </span>
              ) : preview.state === "error" ? (
                <span className="text-danger">{preview.error}</span>
              ) : preview.state === "ready" ? (
                <SharePreviewCard preview={preview.preview} />
              ) : null}
            </ImportCard>
          ) : null}

          {designLink !== "" || (importingDesign && kind === "design") ? (
            <ImportCard
              link={designShare}
              onLink={setDesignShare}
              placeholder="Public Claude artifact or Chauffeur design link…"
              onClose={() => {
                setImportingDesign(false);
                setDesignShare("");
              }}
              testId="import-design-link"
            >
              {claudeImport ? (
                <ClaudeArtifactImport link={designLink} onReady={setClaudeReadyLink} />
              ) : designShare.trim() && !isDesignShareLink(designShare) ? (
                <span className="text-faint">Paste a public Claude artifact (claude.ai/artifact/…) or Chauffeur design link (…/s/&lt;id&gt;).</span>
              ) : (
                <span className="text-faint">
                  Its screens and comments are copied into the new design, for your agent to carry on; the share stays as it is.
                </span>
              )}
            </ImportCard>
          ) : null}

          <div
            {...attachments.dropTarget}
            className="relative rounded-lg border border-line-strong bg-bg focus-within:border-accent/55 focus-within:ring-[3px] focus-within:ring-accent/10"
          >
            {attachments.dropping && <DropOverlay />}
            {slash.menu}
            <ImageStrip images={attachments.images} reading={attachments.reading} onRemove={attachments.remove} className="px-3 pt-2.5" />
            <textarea
              ref={promptRef}
              autoFocus
              value={prompt}
              onChange={(e) => setPrompt(e.target.value)}
              onKeyDown={(e) => void slash.onKeyDown(e.nativeEvent)}
              onPaste={attachments.onPaste}
              rows={4}
              placeholder={
                shareLink
                  ? preview.state === "ready" && preview.preview.chats === 1
                    ? "What next? Your message continues the imported conversation."
                    : "Anything to add for the agent? It reads the imported conversations first."
                  : designChat
                    ? "What would you like to design?"
                    : explore
                      ? "What do you want to understand?"
                      : "What should the agent do?"
              }
              className="block min-h-[104px] w-full resize-none bg-transparent px-3 pt-[11px] pb-1.5 text-[13.5px] leading-normal text-fg placeholder:text-faint focus:outline-none"
              data-testid="first-prompt"
            />
            <div className="flex items-center gap-1 px-1.5 pb-1.5">
              <AttachButton onFiles={(files) => void attachments.addFiles(files)} />
              <ModelPicker value={choice} onChange={setPicked} shortcuts />
              {designChat ? (
                <DesignOnlyMode agent={choice.agent} />
              ) : explore ? (
                <ReadOnlyMode />
              ) : (
                <PermissionPicker value={mode} onChange={setMode} />
              )}
              <span className="ml-auto truncate pr-1 text-[11px] text-faint">
                <Kbd>/</Kbd> skills · paste or drop images
              </span>
            </div>
          </div>

          <div className="grid grid-cols-[84px_minmax(0,1fr)] items-start gap-y-1">
            <PropLabel icon={<Folder size={13} />}>
              {designChat ? "Read from" : "Repos"}
            </PropLabel>
            <PropValue>
              {selected == null ? (
                <span className="inline-flex h-[22px] items-center gap-[5px] rounded-[5px] bg-accent/15 px-[7px] text-[12px] text-accent opacity-60">
                  <Loader2 size={11} className="animate-spin" /> defaults…
                </span>
              ) : (
                selection.map((key) => (
                  <RepoChip key={key} name={key} from={repoSources[key]} onRemove={() => toggleRepo(key)} />
                ))
              )}
              <AddButton
                ref={repoButton}
                open={popover === "repos"}
                onClick={() => setPopover((p) => (p === "repos" ? null : "repos"))}
                testId="add-repo"
              >
                Add repo
              </AddButton>
              {designChat && selection.length > 0 && (
                <Button variant="ghost" size="sm" onClick={() => setChoiceOfRepos({ selection: [], sources: {} })} data-testid="without-repos">
                  Without repo
                </Button>
              )}
              {noRepo ? (
                <span role="status" className="flex items-start gap-1.5 text-[11.5px] text-muted" data-testid="no-repo">
                  <AlertTriangle size={13} className="mt-0.5 shrink-0 text-attention" />
                  {designChat
                    ? "Without a repo, the agent has no code to read your design system from."
                    : "Pick at least one repo to continue."}
                </span>
              ) : (
                designChat && <Hint>the agent reuses their tokens and components</Hint>
              )}
              {popover === "repos" && (
                <RepoPopover anchor={repoButton} onClose={() => setPopover(null)} selection={selection} onToggle={toggleRepo} />
              )}
            </PropValue>

            <PropLabel icon={<Ticket size={13} />}>Tickets</PropLabel>
            <PropValue>
              {tickets.map((ticket) => (
                <TicketChip key={ticket.identifier} ticket={ticket} state={linked[ticket.identifier]?.state ?? null} onRemove={() => removeTicket(ticket.identifier)} />
              ))}
              {milestone && (
                <span className="inline-flex h-[22px] max-w-[300px] items-center gap-[5px] rounded-[5px] border border-line bg-raised pl-[7px] pr-[3px] text-[12px] text-fg" data-testid="milestone-chip">
                  <Diamond size={11} className="shrink-0 text-faint" />
                  <span className="truncate">{milestone.name}</span>
                  <RemoveButton label={`Remove ${milestone.name}`} onClick={() => setMilestone(null)} />
                </span>
              )}
              <AddButton
                ref={ticketButton}
                open={popover === "tickets"}
                onClick={() => setPopover((p) => (p === "tickets" ? null : "tickets"))}
                testId="link-ticket"
              >
                {tickets.length > 0 || milestone ? null : "Link a Linear ticket"}
              </AddButton>
              {tickets.length === 0 && !milestone && kind !== "design" && (
                <Hint>{explore ? "for context only: its status does not change" : "its repos and branch come with it"}</Hint>
              )}
              {popover === "tickets" && (
                <TicketPopover
                  anchor={ticketButton}
                  onClose={() => setPopover(null)}
                  tickets={tickets}
                  milestone={milestone?.id ?? null}
                  onAdd={addTicket}
                  onRemoveLast={() => tickets.length > 0 && removeTicket(tickets[tickets.length - 1].identifier)}
                  onMilestone={(m) => setMilestone({ id: m.id, name: m.name })}
                />
              )}
            </PropValue>

            {kind === "explore" && (
              <>
                <PropLabel icon={<GitBranch size={13} />}>Checkout</PropLabel>
                <PropValue>
                  {checkout ? (
                    <span className="font-mono text-[11.5px] text-muted">{checkout}</span>
                  ) : (
                    <span className="text-muted">each repo's base branch</span>
                  )}
                  <span className="text-faint">· freshly fetched · no setup, no ports</span>
                </PropValue>
              </>
            )}

            {kind === "workspace" && editNames && (
              <>
                <PropLabel icon={<Type size={13} />}>Name</PropLabel>
                <PropValue>
                  <input
                    className="h-[26px] min-w-0 flex-1 rounded-md border border-line bg-bg px-2 text-[12.5px] text-fg placeholder:text-faint focus:border-accent/60 focus:outline-none"
                    placeholder="from the ticket or the first prompt"
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    data-testid="workspace-name"
                  />
                </PropValue>
              </>
            )}
            {kind === "workspace" && (
              <>
                <PropLabel icon={<GitBranch size={13} />} danger={!!error?.branch}>Branch</PropLabel>
                <PropValue>
                  {editNames ? (
                    <div className="flex min-w-0 flex-1 flex-col gap-1">
                      <input
                        className={cx(
                          "h-[26px] w-full rounded-md border bg-bg px-2 font-mono text-[11.5px] text-fg placeholder:text-faint focus:outline-none",
                          error?.branch ? "border-danger/60" : "border-line focus:border-accent/60",
                        )}
                        placeholder={branchToBe.kind === "known" ? branchToBe.branch : branchToBe.kind === "later" ? `${branchToBe.prefix}…` : ""}
                        value={branch}
                        autoFocus={!!error?.branch}
                        onChange={(e) => setBranch(e.target.value)}
                        data-testid="workspace-branch"
                      />
                      {error?.branch?.suggestion && (
                        <span className="text-[11.5px] text-danger">
                          Pick another name, e.g.{" "}
                          <button type="button" className="font-mono text-accent hover:underline" onClick={() => setBranch(error.branch?.suggestion ?? "")}>
                            {error.branch.suggestion}
                          </button>
                        </span>
                      )}
                    </div>
                  ) : (
                    <>
                      {branchToBe.kind === "known" ? (
                        <span className="min-w-0 truncate font-mono text-[11.5px] text-fg" data-testid="branch-preview">{branchToBe.branch}</span>
                      ) : branchToBe.kind === "later" ? (
                        <>
                          <span className="font-mono text-[11.5px] text-muted" data-testid="branch-preview">{branchToBe.prefix}</span>
                          <span className="text-faint">{branchToBe.note}</span>
                        </>
                      ) : null}
                      {branchToBe.kind === "known" && branchToBe.from && <Hint>· {branchToBe.from}</Hint>}
                      <button type="button" className="ml-1 text-[11.5px] text-faint hover:text-fg" onClick={() => setEditNames(true)} data-testid="edit-names">
                        · {branchToBe.kind === "known" ? "Edit" : "Edit name and branch"}
                      </button>
                    </>
                  )}
                </PropValue>
              </>
            )}
          </div>

          {starting.length > 0 && (
            <div className="-mt-0.5 flex items-center gap-1.5 text-[11.5px] text-faint" data-testid="starts-ticket">
              <Info size={12} className="shrink-0" />
              {starting.join(" and ")} {starting.length === 1 ? "moves" : "move"} to In Progress when the workspace is created.
            </div>
          )}

          {!designChat && (office || design) && (
            <Suspense fallback={null}>
              <DesignPicker design={design} onPick={setDesign} />
            </Suspense>
          )}
        </fieldset>
      </div>
    </Modal>
  );
}

/** What a shared workspace's link brings, read before importing it. */
function useSharePreview(link: string, signedIn: boolean):
  | { state: "idle" }
  | { state: "loading" }
  | { state: "error"; error: string }
  | { state: "ready"; preview: WorkspaceSharePreview } {
  const [result, setResult] = useState<
    { link: string; preview: WorkspaceSharePreview } | { link: string; error: string } | null
  >(null);
  const url = link.trim();
  const wanted = signedIn && isWorkspaceShareLink(url);
  useEffect(() => {
    if (!wanted) return;
    let live = true;
    const timer = setTimeout(() => {
      client.call("workspace.share_preview", { url }).then(
        (preview) => { if (live) setResult({ link: url, preview }); },
        (error) => { if (live) setResult({ link: url, error: errorMessage(error) }); },
      );
    }, 250);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [url, wanted]);
  if (!wanted) return { state: "idle" };
  if (!result || result.link !== url) return { state: "loading" };
  return "preview" in result ? { state: "ready", preview: result.preview } : { state: "error", error: result.error };
}

/** What the new workspace is for: one choice in the header, explained on the line below. */
function KindSwitch({ kind: selectedKind, onChange }: { kind: CreateKind; onChange: (kind: CreateKind) => void }) {
  const kinds = [
    { kind: "workspace" as const, icon: <Layers size={13} />, label: "Workspace" },
    { kind: "explore" as const, icon: <Telescope size={13} />, label: "Explore" },
    { kind: "design" as const, icon: <PenTool size={13} />, label: "Design" },
  ];
  return (
    <div className="inline-flex gap-0.5 rounded-[7px] border border-line bg-bg p-0.5 font-normal" role="radiogroup" aria-label="Kind">
      {kinds.map((kind) => {
        const checked = kind.kind === selectedKind;
        return (
          <button
            type="button"
            role="radio"
            aria-checked={checked}
            key={kind.kind}
            onClick={() => onChange(kind.kind)}
            className={cx(
              "inline-flex h-[26px] items-center gap-1.5 rounded-[5px] border px-[11px] text-[12.5px]",
              checked ? "border-line-strong bg-raised font-medium text-fg" : "border-transparent text-muted hover:text-fg",
            )}
            data-testid={`kind-${kind.kind}`}
          >
            <span className={checked ? "text-accent" : undefined}>{kind.icon}</span> {kind.label}
          </button>
        );
      })}
    </div>
  );
}

function PropLabel({ icon, danger, children }: { icon: ReactNode; danger?: boolean; children: ReactNode }) {
  return (
    <div className={cx("flex h-[26px] items-center gap-1.5 whitespace-nowrap text-[12px]", danger ? "text-danger" : "text-faint")}>
      {icon}
      {children}
    </div>
  );
}

function PropValue({ children }: { children: ReactNode }) {
  return <div className="relative flex min-h-[26px] min-w-0 flex-wrap items-center gap-1 text-[12.5px]">{children}</div>;
}

function Hint({ children }: { children: ReactNode }) {
  return <span className="ml-1 text-[11.5px] text-faint">{children}</span>;
}

function RemoveButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button type="button" aria-label={label} className="flex h-4 w-4 shrink-0 items-center justify-center rounded-[3px] opacity-80 hover:bg-hover hover:opacity-100" onClick={onClick}>
      <X size={10} />
    </button>
  );
}

function RepoChip({ name, from, onRemove }: { name: string; from?: string; onRemove: () => void }) {
  return (
    <span className="inline-flex h-[22px] items-center gap-[5px] whitespace-nowrap rounded-[5px] bg-accent/15 pl-[7px] pr-[3px] text-[12px] text-accent" data-repo-chip={name}>
      {name}
      {from && <span className="text-[10.5px] text-accent/70" title={`Named in ${from}'s ## Repo section`}>· {from}</span>}
      <button type="button" aria-label={`Remove ${name}`} className="flex h-4 w-4 items-center justify-center rounded-[3px] opacity-80 hover:bg-accent/25 hover:opacity-100" onClick={onRemove}>
        <X size={10} />
      </button>
    </span>
  );
}

function TicketChip({ ticket, state, onRemove }: { ticket: TicketRef; state: string | null; onRemove: () => void }) {
  return (
    <span
      title={ticket.title ?? undefined}
      className="inline-flex h-[22px] min-w-0 max-w-[380px] items-center gap-[5px] rounded-[5px] border border-line bg-raised pl-[7px] pr-[3px] text-[12px] text-fg"
      data-ticket-chip={ticket.identifier}
    >
      <StateDot state={state} />
      <b className="shrink-0 font-medium">{ticket.identifier}</b>
      {ticket.title && <span className="min-w-0 truncate text-muted">{ticket.title}</span>}
      <RemoveButton label={`Remove ${ticket.identifier}`} onClick={onRemove} />
    </span>
  );
}

/** A dashed "+" that opens a picker; solid while its picker is open. */
function AddButton({
  ref,
  open,
  danger,
  onClick,
  testId,
  children,
}: {
  ref: React.Ref<HTMLButtonElement>;
  open: boolean;
  danger?: boolean;
  onClick: () => void;
  testId: string;
  children: ReactNode;
}) {
  return (
    <button
      ref={ref}
      type="button"
      aria-expanded={open}
      aria-label={children ? undefined : "Add"}
      onClick={onClick}
      className={cx(
        "inline-flex h-[22px] items-center gap-1 rounded-[5px] border px-[7px] text-[12px]",
        open ? "border-line-strong bg-hover text-fg" : "border-dashed text-faint hover:text-fg",
        danger ? "border-danger/55" : !open && "border-line-strong",
      )}
      data-testid={testId}
    >
      <Plus size={11} />
      {children}
    </button>
  );
}

/** The link of a colleague's share being imported, with what it brings below. */
function ImportCard({
  link,
  onLink,
  placeholder,
  onClose,
  testId,
  children,
}: {
  link: string;
  onLink: (link: string) => void;
  placeholder: string;
  onClose: () => void;
  testId: string;
  children: ReactNode;
}) {
  return (
    <div className="rounded-lg border border-line-strong bg-raised">
      <div className="flex items-center gap-2 border-b border-line px-[11px] py-[7px]">
        <Link2 size={13} className="shrink-0 text-faint" />
        <input
          autoFocus={!link}
          className="h-6 min-w-0 flex-1 bg-transparent font-mono text-[11.5px] text-fg placeholder:text-faint focus:outline-none"
          placeholder={placeholder}
          value={link}
          onChange={(e) => onLink(e.target.value)}
          data-testid={testId}
        />
        <RemoveButton label="Do not import" onClick={onClose} />
      </div>
      <div className="px-[11px] py-[9px] text-[12px]">{children}</div>
    </div>
  );
}

function SharePreviewCard({ preview }: { preview: WorkspaceSharePreview }) {
  const owner = preview.owner.name || preview.owner.email;
  const pushed = preview.last_push_at ? relativeTime(new Date(preview.last_push_at).toISOString()) : "";
  return (
    <div className="flex items-start gap-2.5" data-testid="share-preview">
      <span className="flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-full bg-accent/25 text-[11px] font-semibold text-accent">
        {owner.slice(0, 1).toUpperCase()}
      </span>
      <div className="min-w-0 text-[12.5px]">
        <div>
          <b className="font-medium text-fg">{preview.title}</b>{" "}
          <span className="text-faint">· shared by {owner}{pushed && ` · ${pushed}`}</span>
        </div>
        <div className="text-[12px] text-muted">
          {preview.chats} conversation{preview.chats === 1 ? "" : "s"}
          {preview.notes > 0 && ` and ${preview.notes} note${preview.notes === 1 ? "" : "s"}`} {preview.chats + preview.notes === 1 ? "is" : "are"} copied
          once into new chats. Code changes and design assets stay behind: pick the repos below.{" "}
          {preview.chats === 1 ? "Your first message continues that conversation." : "Your first message opens a chat of its own."}
        </div>
      </div>
    </div>
  );
}

/** Linear's tickets and milestones to link, opened from the Tickets row. */
function TicketPopover({
  anchor,
  onClose,
  tickets,
  milestone,
  onAdd,
  onRemoveLast,
  onMilestone,
}: {
  anchor: React.RefObject<HTMLElement | null>;
  onClose: () => void;
  tickets: TicketRef[];
  milestone: string | null;
  onAdd: (ticket: TicketRef, repos: string[], issue: LinearIssue | null) => void;
  onRemoveLast: () => void;
  onMilestone: (milestone: LinearMilestone) => void;
}) {
  const search = useTicketSearch(new Set(tickets.map((t) => t.identifier)), onAdd, { attached: milestone, onPick: onMilestone });
  const { setOpen, q, list, milestones, loading, linearError, highlight } = search;
  useEffect(() => setOpen(true), [setOpen]);
  const row = (index: number) =>
    cx("flex h-[30px] w-full min-w-0 items-center gap-2 px-2.5 text-left text-[12.5px]", index === highlight && "bg-hover");

  return (
    <Popover anchor={anchor} width={520} height={360} onClose={onClose} testId="ticket-popover">
      <div className="flex h-[34px] shrink-0 items-center gap-[7px] border-b border-line px-2.5">
        <Search size={13} className="shrink-0 text-faint" />
        <input
          data-autofocus
          className="h-full min-w-0 flex-1 bg-transparent text-[12.5px] text-fg placeholder:text-faint focus:outline-none"
          placeholder="Search Linear, or paste a ticket, parent issue or milestone link…"
          value={search.query}
          onChange={(e) => search.setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Backspace" && !search.query && !e.metaKey && !e.ctrlKey) {
              onRemoveLast();
              return;
            }
            search.onKeyDown(e);
          }}
          data-testid="ticket-input"
        />
        {loading && <Loader2 size={12} className="shrink-0 animate-spin text-faint" />}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto pb-1" data-testid="ticket-results">
        {list.length > 0 && (
          <div className="px-2.5 pt-2 pb-[3px] text-[10.5px] font-medium uppercase tracking-[.05em] text-faint">
            {q ? "All tickets" : "Assigned to you"}
          </div>
        )}
        {list.map((issue, index) => (
          <button
            type="button"
            key={issue.identifier}
            onMouseDown={(e) => {
              e.preventDefault();
              search.pick(issue);
            }}
            onMouseEnter={() => search.setHighlight(index)}
            className={row(index)}
            data-ticket={issue.identifier}
          >
            <StateDot state={issue.state} />
            <b className="w-[70px] shrink-0 font-medium text-fg">{issue.identifier}</b>
            <span className="min-w-0 max-w-[230px] truncate text-fg">{issue.title}</span>
            <span className="ml-auto min-w-0 truncate pl-2 text-[11.5px] text-faint">{issue.repos.join(" · ")}</span>
          </button>
        ))}
        {milestones.length > 0 && (
          <div className="px-2.5 pt-2 pb-[3px] text-[10.5px] font-medium uppercase tracking-[.05em] text-faint">Milestones</div>
        )}
        {milestones.map((m, i) => {
          const index = list.length + i;
          return (
            <button
              type="button"
              key={m.id}
              onMouseDown={(e) => {
                e.preventDefault();
                search.pickOption({ kind: "milestone", milestone: m });
              }}
              onMouseEnter={() => search.setHighlight(index)}
              className={row(index)}
              data-milestone={m.id}
            >
              <Diamond size={12} className="shrink-0 text-faint" />
              <b className="shrink-0 font-medium text-fg">{m.name}</b>
              <span className="min-w-0 truncate text-faint">{m.project}</span>
              <span className="ml-auto shrink-0 pl-2 text-[11.5px] text-faint">
                {m.open_issues} ticket{m.open_issues === 1 ? "" : "s"} · attached together
              </span>
            </button>
          );
        })}
        {list.length === 0 && milestones.length === 0 && (
          <div className="px-2.5 py-2 text-[12px] text-faint">
            {loading
              ? "Searching Linear…"
              : linearError
                ? `Linear is unavailable: ${linearError}. Type a ticket id (TEAM-123) and press ↵ to link it.`
                : q
                  ? `No ticket or milestone matches “${q}”.`
                  : "No open ticket is assigned to you."}
          </div>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-3 border-t border-line px-2.5 py-1.5 text-[11px] text-faint">
        <span className="flex items-center gap-1"><Kbd>↵</Kbd> link</span>
        <span>
          Repos named in a ticket's <span className="font-mono text-[10.5px]">## Repo</span> section are added
        </span>
      </div>
    </Popover>
  );
}
