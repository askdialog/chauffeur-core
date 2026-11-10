// The settings window (⌘,): what most people change, by section. The daemon writes each change to
// ~/.chauffeur/settings.toml (keeping its comments), the API keys to the Keychain, and applies them
// at once; the rest of settings.toml opens in the editor from General.

import type { AppSettings, ErrorReports, KeepAwake, OllamaPull, Palette, RepoSetting, SearchStatus, SettingsUpdate, ShortcutId, UpdateStatus } from "@chauffeur/protocol";
import { ArrowRightLeft, Bell, CircleUser, Bot, Coffee, Cpu, ExternalLink, FileText, FolderGit2, GitBranch, Info, Keyboard, KeyRound, Layers, Play, Plus, Power, Package, Search, Settings2, Smartphone, Type, X } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useState } from "react";
import { effectiveLoadout, type ModelChoice } from "@/lib/models";
import { GithubSetup } from "./GithubSetup";
import { LocalModelsSettings } from "./LocalModelsSettings";
import { TransferSettings } from "./TransferSettings";
import { WorkerSettings } from "./WorkerSettings";
import { SlackSettings } from "./SlackSettings";
import { ExtensionMarketplace } from "./ExtensionMarketplace";
import { MobileSettings } from "./MobileSettings";
import { AccountSettings } from "./AccountSettings";
import { MEMBERS_ONLY_SECTIONS } from "@/lib/members";
import { errorMessage } from "@/lib/rpc";
import { setErrorReports } from "@/state/errorReports";
import { secretStore } from "@/lib/secretStore";
import { daemonMismatch, updateNote } from "@/lib/about";
import { pullLabel } from "@/lib/localModels";
import { semanticLine } from "@/lib/search";
import { openExternal, setMenuKeys } from "@/lib/native";
import { version as APP_VERSION } from "../../package.json";
import appIcon from "../../src-tauri/icons/128x128@2x.png";
import {
  client,
  loadLoadout,
  loadModels,
  loadRepos,
  openSettings,
  playNotificationSound,
  setAppearance,
  setKeyboard,
  setNotificationSound,
  toast,
  useApp,
} from "@/state/app";
import { RepoOrder } from "./RepoOrder";
import { ModelPicker } from "./ModelPicker";
import { LlmFavorites } from "./LlmFavorites";
import { isMac, kbd } from "@/lib/keys";
import { BUILT_IN_SOUNDS } from "@/lib/sounds";
import { conflictsOf, keysOf, labelOf, rebind, recordBinding, RESERVED, SHORTCUT_IDS, SHORTCUTS } from "@/lib/shortcuts";
import { DEFAULT_CODE_SIZE, DEFAULT_MONO, DEFAULT_SANS, DEFAULT_TERMINAL_SIZE, fontStack, MONO_SUGGESTIONS, SANS_SUGGESTIONS } from "@/lib/fonts";
import { interfaceHint } from "@/lib/palette";
import { Button, cx, Field, IconButton, inputClass, Modal, Spinner, Toggle } from "./ui";

type Section = "account" | "extensions" | "slack" | "transfer" | "mobile" | "general" | "github" | "daily-review" | "appearance" | "keyboard" | "notifications" | "workspaces" | "llms" | "agents" | "local-models" | "search" | "sleep" | "linear" | "compaction" | "about";

const SECTIONS: Array<{ id: Section; label: string; icon: ReactNode }> = [
  { id: "account", label: "Account", icon: <CircleUser size={13} /> },
  { id: "extensions", label: "Extensions", icon: <Package size={13} /> },
  { id: "transfer", label: navigator.language.startsWith("fr") ? "Transferts" : "Transfers", icon: <ArrowRightLeft size={13} /> },
  { id: "slack", label: "Slack", icon: <Layers size={13} /> },
  { id: "mobile", label: "Mobile", icon: <Smartphone size={13} /> },
  { id: "general", label: "General", icon: <Settings2 size={13} /> },
  { id: "github", label: "GitHub", icon: <GitBranch size={13} /> },
  { id: "appearance", label: "Appearance", icon: <Type size={13} /> },
  { id: "keyboard", label: "Keyboard", icon: <Keyboard size={13} /> },
  { id: "notifications", label: "Notifications", icon: <Bell size={13} /> },
  { id: "workspaces", label: "Workspaces", icon: <FolderGit2 size={13} /> },
  { id: "llms", label: "LLMs", icon: <Settings2 size={13} /> },
  { id: "daily-review", label: "Daily review", icon: <FileText size={13} /> },
  { id: "agents", label: "Agents", icon: <Bot size={13} /> },
  { id: "local-models", label: "Local models", icon: <Cpu size={13} /> },
  { id: "search", label: "Search", icon: <Search size={13} /> },
  { id: "sleep", label: "Sleep", icon: <Coffee size={13} /> },
  { id: "linear", label: "Linear", icon: <Layers size={13} /> },
  { id: "compaction", label: "Compaction", icon: <KeyRound size={13} /> },
  { id: "about", label: "About Chauffeur", icon: <Info size={13} /> },
];

const SECTION_GROUPS: Array<{ label: string; sections: Section[] }> = [
  { label: "Preferences", sections: ["general", "appearance", "keyboard", "notifications", "sleep"] },
  { label: "Work", sections: ["workspaces", "agents", "llms", "local-models", "search", "compaction", "daily-review"] },
  { label: "Connections", sections: ["account", "transfer", "mobile", "github", "linear", "slack", "extensions"] },
  { label: "Application", sections: ["about"] },
];

type Update = (change: SettingsUpdate) => Promise<boolean>;

export function SettingsDialog() {
  const open = useApp((s) => s.settingsOpen);
  const askedSection = useApp((s) => s.settingsSection);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [section, setSection] = useState<Section>("general");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [search, setSearch] = useState("");
  const member = useApp((s) => s.identity?.member ?? false);
  const offered = (id: Section) => member || !MEMBERS_ONLY_SECTIONS.includes(id);

  useEffect(() => {
    if (open && askedSection && SECTIONS.some((s) => s.id === askedSection && offered(s.id))) setSection(askedSection as Section);
  }, [open, askedSection]);
  // Signed out of a member's account while one of their sections shows.
  useEffect(() => {
    if (!offered(section)) setSection("account");
  }, [member, section]);

  useEffect(() => {
    if (!open) return;
    setError(null);
    client
      .call("settings.get", {})
      .then(setSettings)
      .catch((err) => setError(errorMessage(err)));
  }, [open]);

  const update: Update = async (change) => {
    setBusy(true);
    setError(null);
    try {
      const next = await client.call("settings.set", change);
      setSettings(next);
      setErrorReports(next);
      setAppearance(next.appearance);
      setNotificationSound(next.notifications.sound);
      setKeyboard(next.keyboard);
      // The create dialog's preselected repos.
      if (change.workspaces) await loadRepos();
      return true;
    } catch (err) {
      setError(errorMessage(err));
      return false;
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal open={open} onClose={() => openSettings(false)} title="Settings" width={720} height={600}>
      <div className="flex h-full min-h-0 gap-5">
        <nav className="flex w-36 shrink-0 flex-col gap-0.5 overflow-y-auto" aria-label="Settings sections">
          <input type="search" aria-label="Search settings" placeholder="Search settings…" value={search} onChange={(event) => setSearch(event.target.value)} className={cx(inputClass, "mb-2 text-[12px]")} />
          {SECTION_GROUPS.map((group) => {
            const items = group.sections.filter(offered).map((id) => SECTIONS.find((s) => s.id === id)!)
              .filter((s) => `${s.label} ${s.id} ${group.label}`.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()));
            if (!items.length) return null;
            return <div key={group.label} className="mb-2">
              <p className="mb-1 px-2 text-[11px] font-medium text-muted">{group.label}</p>
              {items.map((s) => (
            <button
              key={s.id}
              onClick={() => {
                setSection(s.id);
                setError(null);
              }}
              aria-current={section === s.id ? "page" : undefined}
              className={cx(
                "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12.5px]",
                section === s.id ? "bg-hover text-fg" : "text-muted hover:bg-hover/60 hover:text-fg",
              )}
              data-testid={`settings-${s.id}`}
            >
              {s.icon}
              {s.label}
            </button>
          ))}
            </div>;
          })}
          {search.trim() && !SECTIONS.some((s) => offered(s.id) && `${s.label} ${s.id}`.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase())) && !SECTION_GROUPS.some((g) => g.label.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase())) && <p className="px-2 text-[12px] text-muted">No matching settings.</p>}
        </nav>
        <div key={section} className="min-w-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]">
          {!settings ? (
            <div className="flex items-center gap-2 text-[12.5px] text-muted">
              {error ? <span className="text-danger">{error}</span> : <Spinner />}
            </div>
          ) : (
            <div className="space-y-4">
              {settings.file_error && (
                <p className="rounded-md border border-attention/30 bg-attention/10 px-3 py-2 text-[12px] text-attention selectable">
                  settings.toml does not parse, so its last valid version is in force: {settings.file_error}
                </p>
              )}
              {section === "transfer" && (
                <div className="space-y-8">
                  <TransferSettings settings={settings} update={update} busy={busy} />
                  <WorkerSettings key={`${settings.workers?.project}:${settings.workers?.zone}`} />
                </div>
              )}
              {section === "account" && <AccountSettings />}
              {section === "extensions" && <ExtensionMarketplace />}
              {section === "slack" && member && <SlackSettings />}
              {section === "mobile" && <MobileSettings />}
              {section === "general" && <General settings={settings} update={update} busy={busy} onError={setError} />}
              {section === "github" && (
                <>
                  <Heading title="GitHub">Chauffeur creates workspaces from your GitHub repositories and opens their pull requests with your account. Git pushes through it too: no SSH key needed.</Heading>
                  <GithubSetup manage />
                </>
              )}
              {section === "appearance" && <AppearanceSection settings={settings} update={update} busy={busy} />}
              {section === "keyboard" && <KeyboardSection settings={settings} update={update} busy={busy} />}
              {section === "notifications" && <Notifications settings={settings} update={update} busy={busy} />}
              {section === "workspaces" && <Workspaces settings={settings} update={update} busy={busy} />}
              {section === "llms" && <Llms settings={settings} update={update} busy={busy} />}
              {section === "daily-review" && <DailyReview settings={settings} update={update} busy={busy} />}
              {section === "agents" && <Agents settings={settings} update={update} busy={busy} />}
              {section === "local-models" && <LocalModelsSettings />}
              {section === "search" && <SearchSection settings={settings} update={update} busy={busy} />}
              {section === "sleep" && <Sleep settings={settings} update={update} busy={busy} />}
              {section === "linear" && <Linear settings={settings} update={update} busy={busy} />}
              {section === "compaction" && <Compaction settings={settings} update={update} busy={busy} />}
              {section === "about" && <About />}
              {error && <p className="text-[12px] text-danger selectable">{error}</p>}
            </div>
          )}
        </div>
      </div>
    </Modal>
  );
}

type SectionProps = { settings: AppSettings; update: Update; busy: boolean };

function General({ settings, update, busy, onError }: SectionProps & { onError: (error: string | null) => void }) {
  const [opening, setOpening] = useState(false);
  const openFile = async () => {
    setOpening(true);
    try {
      await client.call("settings.open", {});
    } catch (err) {
      onError(errorMessage(err));
    } finally {
      setOpening(false);
    }
  };
  return (
    <>
      <Heading title="General" />
      <Switch
        label="Start Chauffeur at login"
        hint="The daemon is up as soon as you log in (for the CLI and Linear's Work on issue, without opening the app) and restarts if it crashes. From the next login on."
        on={settings.start_at_login}
        disabled={busy}
        onChange={(on) => void update({ start_at_login: on })}
        testId="start-at-login"
        icon={<Power size={13} />}
      />
      {settings.error_reports_available && <ErrorReportsChoice settings={settings} update={update} busy={busy} />}
      <Field label="Daemon port" hint="from the daemon's next start">
        <NumberField value={settings.daemon_port} min={1024} max={65535} busy={busy} onSave={(v) => update({ daemon_port: v })} testId="daemon-port" />
      </Field>
      <Field label="settings.toml" hint="these settings, as a file; applied within a second">
        <div className="flex items-center gap-2">
          <span className={cx(inputClass, "flex items-center truncate text-faint")} title={settings.file}>
            {settings.file}
          </span>
          <Button size="sm" onClick={() => void openFile()} busy={opening} data-testid="open-settings-file">
            <FileText size={12} /> Open
          </Button>
        </div>
      </Field>
    </>
  );
}

const ERROR_REPORTS: { id: ErrorReports; label: string }[] = [
  { id: "always", label: "Always send" },
  { id: "ask", label: "Ask each time" },
  { id: "never", label: "Never send" },
];

/** Whether the errors shown as toasts are reported to Chauffeur's developers (`error_reports`). */
function ErrorReportsChoice({ settings, update, busy }: SectionProps) {
  return (
    <Field label="Error reports" hint="when Chauffeur shows an error">
      <div className="flex gap-1.5" role="radiogroup" aria-label="Error reports">
        {ERROR_REPORTS.map((c) => (
          <button
            key={c.id}
            type="button"
            role="radio"
            aria-checked={settings.error_reports === c.id}
            disabled={busy}
            onClick={() => c.id !== settings.error_reports && void update({ error_reports: c.id })}
            className={cx(
              "rounded-md border px-2.5 py-1 text-[12.5px] disabled:opacity-60",
              settings.error_reports === c.id
                ? "border-accent/60 bg-accent/10 text-fg"
                : "border-line bg-raised text-muted hover:border-line-strong",
            )}
            data-testid={`error-reports-${c.id}`}
          >
            {c.label}
          </button>
        ))}
      </div>
      <p className="mt-1.5 text-[11.5px] leading-snug text-faint">
        Sent to Chauffeur's developers (Sentry): the error, the versions and the daemon's log of the failed call,
        without your home folder nor anything like a token, under an anonymous id.
      </p>
    </Field>
  );
}

function Workspaces({ settings, update, busy }: SectionProps) {
  const reposError = useApp((s) => s.reposError);
  const repos = useApp((s) => s.repos);
  useEffect(() => {
    if (!repos) void loadRepos();
  }, [repos]);
  const { default_repos, branch_prefix, github_login, clone_roots } = settings.workspaces;
  const w = settings.workspaces;
  const [org, setOrg] = useState(w.github_org);
  useEffect(() => setOrg(w.github_org), [w.github_org]);
  const saveOrg = () => {
    if (org.trim() !== w.github_org) void update({ workspaces: { github_org: org.trim() } });
  };
  const [prefix, setPrefix] = useState(branch_prefix);
  useEffect(() => setPrefix(branch_prefix), [branch_prefix]);
  const savePrefix = () => {
    if (prefix.trim() !== branch_prefix) void update({ workspaces: { branch_prefix: prefix.trim() } });
  };
  const example = `${prefix.trim() || github_login || "your-github-login"}/fix-price-filter`;

  return (
    <>
      <Heading title="Workspaces" />
      <Field label="Repos a new workspace starts with" hint="you can change them when you create one">
        <ListEditor
          values={default_repos}
          suggestions={(repos ?? []).map((r) => r.key)}
          placeholder="Add a repo…"
          disabled={busy}
          onChange={(values) => void update({ workspaces: { default_repos: values } })}
          testId="default-repos"
        />
      </Field>
      <RepoOrder repos={repos} order={w.repo_order} busy={busy} error={reposError} onRefresh={() => loadRepos(true)}
        onSave={(repo_order) => void update({ workspaces: { repo_order } })} />
      <Field label="Branch prefix" hint={<span className="font-mono">{example}</span>}>
        <input
          value={prefix}
          onChange={(event) => setPrefix(event.target.value)}
          onBlur={savePrefix}
          onKeyDown={(event) => {
            if (event.key === "Enter") savePrefix();
          }}
          placeholder={github_login ? `${github_login} (your GitHub login)` : "your GitHub login"}
          spellCheck={false}
          className={inputClass}
          data-testid="branch-prefix"
        />
      </Field>
      <Field label="Folders with your clones" hint="a repo found there is reused rather than cloned">
        <ListEditor
          values={clone_roots}
          placeholder="Add a folder, e.g. ~/code"
          disabled={busy}
          mono
          onChange={(values) => void update({ workspaces: { clone_roots: values } })}
          testId="clone-roots"
        />
      </Field>
      <Field label="Main GitHub organization" hint="its repos go without their owner's name">
        <input
          value={org}
          onChange={(event) => setOrg(event.target.value)}
          onBlur={saveOrg}
          onKeyDown={(event) => {
            if (event.key === "Enter") saveOrg();
          }}
          spellCheck={false}
          className={inputClass}
          data-testid="github-org"
        />
      </Field>
      <Switch
        label="Every repo my GitHub account reaches"
        hint="Your own, your other organizations' and the ones you collaborate on, beside the main organization's."
        on={w.github_sources}
        disabled={busy}
        onChange={(on) => void update({ workspaces: { github_sources: on } }).then(() => loadRepos(true))}
        testId="github-sources"
      />
      <Field label="Repos elsewhere" hint="a clone outside these folders, or another default branch">
        <RepoTable repos={w.repos} busy={busy} onSave={(repos) => update({ workspaces: { repos } })} />
      </Field>
      <Field label="Ports" hint="a block per new workspace (CHAUFFEUR_PORT)">
        <div className="flex items-center gap-2 text-[12.5px] text-muted">
          <span>from</span>
          <NumberField value={w.port_range_start} min={1024} max={64000} busy={busy} onSave={(v) => update({ workspaces: { port_range_start: v } })} testId="port-range-start" />
          <span>by blocks of</span>
          <NumberField value={w.port_block_size} min={1} max={100} busy={busy} onSave={(v) => update({ workspaces: { port_block_size: v } })} testId="port-block-size" />
        </div>
      </Field>
      <Field label="Child workspaces" hint="at most, at once, per workspace">
        <NumberField value={w.max_children} min={0} max={50} busy={busy} onSave={(v) => update({ workspaces: { max_children: v } })} testId="max-children" />
      </Field>
      <Field label="Archiving" hint="archive scripts' time limit; snapshots kept">
        <div className="flex items-center gap-2 text-[12.5px] text-muted">
          <NumberField value={w.archive_script_timeout_secs} min={10} max={3600} busy={busy} onSave={(v) => update({ workspaces: { archive_script_timeout_secs: v } })} testId="archive-timeout" />
          <span>seconds,</span>
          <NumberField value={w.snapshot_retention_days} min={1} max={365} busy={busy} onSave={(v) => update({ workspaces: { snapshot_retention_days: v } })} testId="snapshot-days" />
          <span>days</span>
        </div>
      </Field>
    </>
  );
}

/** A number saved when the field loses the focus or on Enter; one out of range goes back. */
function NumberField({
  value,
  min,
  max,
  busy,
  onSave,
  testId,
}: {
  value: number;
  min: number;
  max: number;
  busy: boolean;
  onSave: (value: number) => Promise<boolean>;
  testId: string;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const save = () => {
    const next = Number(text);
    if (next === value) return;
    if (!Number.isInteger(next) || next < min || next > max) return setText(String(value));
    void onSave(next);
  };
  return (
    <span className="inline-block w-24 shrink-0">
      <input
        type="number"
        min={min}
        max={max}
        value={text}
        onChange={(event) => setText(event.target.value)}
        onBlur={save}
        onKeyDown={(event) => {
          if (event.key === "Enter") save();
        }}
        disabled={busy}
        className={inputClass}
        data-testid={testId}
      />
    </span>
  );
}

/** `[repos.<name>]`: a row per repo, saved as a whole when a row changes or goes. */
function RepoTable({ repos, busy, onSave }: { repos: RepoSetting[]; busy: boolean; onSave: (repos: RepoSetting[]) => Promise<boolean> }) {
  const [rows, setRows] = useState(repos);
  useEffect(() => setRows(repos), [repos]);
  const save = (next: RepoSetting[]) => {
    const cleaned = next.filter((r) => r.name.trim());
    if (JSON.stringify(cleaned) !== JSON.stringify(repos)) void onSave(cleaned);
  };
  const edit = (i: number, change: Partial<RepoSetting>) => setRows((current) => current.map((r, j) => (j === i ? { ...r, ...change } : r)));
  return (
    <div className="space-y-1.5" data-testid="repo-table">
      {rows.map((repo, i) => (
        <div key={i} className="flex items-center gap-1.5">
          {(
            [
              ["name", "repo", "w-32 shrink-0"],
              ["path", "its clone, e.g. ~/code/repo", "flex-1"],
              ["default_branch", "default branch", "w-28 shrink-0"],
            ] as const
          ).map(([key, placeholder, width]) => (
            <span key={key} className={cx("min-w-0", width)}>
            <input
              value={repo[key] ?? ""}
              placeholder={placeholder}
              onChange={(event) => edit(i, { [key]: key === "name" ? event.target.value : event.target.value || null })}
              onBlur={() => save(rows)}
              onKeyDown={(event) => {
                if (event.key === "Enter") save(rows);
              }}
              disabled={busy}
              spellCheck={false}
              className={cx(inputClass, "font-mono text-[12px]")}
            />
            </span>
          ))}
          <IconButton
            title={`Remove ${repo.name || "this row"}`}
            onClick={() => {
              const next = rows.filter((_, j) => j !== i);
              setRows(next);
              save(next);
            }}
            disabled={busy}
          >
            <X size={12} />
          </IconButton>
        </div>
      ))}
      <Button size="sm" variant="ghost" onClick={() => setRows((current) => [...current, { name: "", path: null, default_branch: null }])} disabled={busy}>
        <Plus size={12} /> Add a repo
      </Button>
    </div>
  );
}

function Llms({ settings, update, busy }: SectionProps) {
  const catalog = useApp((s) => s.models);
  const saved = useApp((s) => s.loadout);
  useEffect(() => {
    if (!catalog) void loadModels();
    if (!saved) void loadLoadout();
  }, [catalog, saved]);
  const loadout = useMemo(() => effectiveLoadout(saved, catalog), [saved, catalog]);
  const first = loadout[0];

  return (
    <>
      <Heading title="LLMs" />
      <LlmFavorites />
      <Switch
        label="Another model for chauffeur new"
        hint="For chats the CLI starts without --model; the app's chats use the first favorite."
        on={!!settings.default_agent}
        disabled={busy || !catalog || !first}
        onChange={(on) =>
          void update(
            on && first
              ? { default_agent: first.agent, default_model: first.model ?? "" }
              : { default_agent: "", default_model: "" },
          )
        }
        testId="cli-model-switch"
      />
      {settings.default_agent && catalog && (
        <Field label="chauffeur new uses">
          <div data-testid="cli-model">
            <ModelPicker
              value={{ agent: settings.default_agent as ModelChoice["agent"], model: settings.default_model ?? null, effort: null, fast: false }}
              onChange={(choice) => void update({ default_agent: choice.agent, default_model: choice.model ?? "" })}
              field
            />
          </div>
        </Field>
      )}
    </>
  );
}

function DailyReview({ settings, update, busy }: SectionProps) {
  const review = settings.daily_review;
  const saved = useApp((s) => s.loadout);
  const catalog = useApp((s) => s.models);
  useEffect(() => {
    if (!catalog) void loadModels();
    if (!saved) void loadLoadout();
  }, [catalog, saved]);
  const first = effectiveLoadout(saved, catalog)[0];
  return <>
    <Heading title="Daily review" />
    {settings.daily_review_error && <p className="text-[12px] text-danger selectable">{settings.daily_review_error}</p>}
    <Switch label="Review my sessions every day at 18:00"
      hint="Uses your local time. On days with session activity, opens a dedicated exploration to review code, Linear, GitHub and opportunities to improve. Missed reviews run when Chauffeur is back."
      on={review.enabled} disabled={busy} testId="daily-review-enabled"
      onChange={(enabled) => void update({ daily_review: { ...review, enabled } })} />
    <Switch label="Use another model for daily reviews"
      hint="Otherwise uses your default agent and model, or your first favorite."
      on={review.agent !== null} disabled={busy || !first} testId="daily-review-model-override"
      onChange={(on) => void update({ daily_review: { ...review, agent: on && first ? first.agent : null, model: on && first ? first.model : null } })} />
    {review.agent && <Field label="Review model">
      <ModelPicker field disabled={busy}
        value={{ agent: review.agent, model: review.model, effort: null, fast: false }}
        onChange={(choice) => void update({ daily_review: { ...review, agent: choice.agent, model: choice.model } })} />
    </Field>}
    <p className="text-[12px] text-muted">The report appears in its workspace. Reviews analyze your sessions and propose changes; they do not apply them.</p>
    <Button size="sm" variant="ghost" onClick={() => openSettings(true, "sleep")}>Configure wake in Sleep settings</Button>
  </>;
}

function Agents({ settings, update, busy }: SectionProps) {
  return (
    <>
      <Heading title="Agents" />
      <Field label="Idle agents" hint="minutes before an idle agent frees its memory; 0 never">
        <NumberField
          value={settings.workspaces.idle_session_minutes}
          min={0}
          max={1440}
          busy={busy}
          onSave={(v) => update({ workspaces: { idle_session_minutes: v } })}
          testId="idle-minutes"
        />
      </Field>
      <Switch
        label="Suggest my next message"
        hint="After each turn, Claude guesses what you will ask next and offers it in the empty composer: Tab takes it, Esc sets it aside."
        on={settings.prompt_suggestions}
        disabled={busy}
        onChange={(on) => void update({ prompt_suggestions: on })}
        testId="prompt-suggestions"
      />
    </>
  );
}

const KEEP_AWAKE: Array<{ id: KeepAwake; label: string; detail: string }> = [
  {
    id: "never",
    label: "Never",
    detail: "The machine sleeps as it would without Chauffeur; agents and their commands pause until it wakes.",
  },
  {
    id: "while_working",
    label: "While agents work",
    detail: "No idle sleep while an agent works, and 2 minutes after. The display may sleep; closing a laptop's lid still sleeps it.",
  },
  {
    id: "lid_closed",
    label: "Even with the lid closed",
    detail: "No sleep at all while an agent works, the lid closed included; sleep comes back when they stop.",
  },
];

/** `keep_awake`: never, while agents work, or even with the lid closed (macOS). */
function Sleep({ settings, update, busy }: SectionProps) {
  const power = settings.power;
  const [pending, setPending] = useState<KeepAwake | null>(null);
  const choose = async (mode: KeepAwake) => {
    if (mode === power.keep_awake || busy) return;
    setPending(mode);
    await update({ power: { keep_awake: mode } });
    setPending(null);
  };
  const choices = KEEP_AWAKE.filter((c) => c.id !== "lid_closed" || power.lid_closed_available);
  // Installing or removing the sudoers rule waits for macOS's password dialog.
  const asking =
    pending !== null && (pending === "lid_closed" ? !power.sudoers_rule : power.keep_awake === "lid_closed" && power.sudoers_rule);
  return (
    <>
      <Heading title="Sleep">
        When the machine sleeps, the agents and the commands they run stop until it wakes. Chauffeur can keep it awake
        while they work.
      </Heading>
      <Switch label="Wake for the daily review at 18:00"
        hint={isMac ? "Requires administrator permission once. Only schedules a wake after session activity, when daily reviews are enabled and sleep is not set to Never. Low battery cancels it; closed-lid wake depends on your Mac and power supply. Otherwise reviews catch up after wake." : "Scheduled wake is available on macOS. Reviews catch up when this computer wakes."}
        on={settings.daily_review.wake} disabled={busy || !isMac} testId="daily-review-wake"
        onChange={(wake) => void update({ daily_review: { ...settings.daily_review, wake } })} />
      <div className="space-y-1.5" role="radiogroup" aria-label="Keep the machine awake">
        {choices.map((c) => (
          <button
            key={c.id}
            type="button"
            role="radio"
            aria-checked={power.keep_awake === c.id}
            disabled={busy}
            onClick={() => void choose(c.id)}
            className={cx(
              "flex w-full flex-col rounded-md border px-3 py-2 text-left disabled:opacity-60",
              power.keep_awake === c.id ? "border-accent/60 bg-accent/10" : "border-line bg-raised hover:border-line-strong",
            )}
            data-testid={`keep-awake-${c.id}`}
          >
            <span className={cx("flex items-center gap-2 text-[12.5px]", power.keep_awake === c.id ? "text-fg" : "text-muted")}>
              {c.label}
              {pending === c.id && <Spinner />}
            </span>
            <span className="text-[11.5px] leading-snug text-faint">{c.detail}</span>
          </button>
        ))}
      </div>
      {asking && (
        <p className="text-[12px] text-attention" data-testid="admin-prompt-hint">
          macOS asks for an administrator's password, in its own window: answer it there.
        </p>
      )}
      {power.lid_closed_available && (
        <>
          <p className="text-[11.5px] leading-relaxed text-faint">
            Even with the lid closed: choosing it asks once for an administrator's password, to install a rule
            (/etc/sudoers.d/chauffeur-pmset) that lets Chauffeur run only <span className="font-mono">pmset -a disablesleep 1</span> and{" "}
            <span className="font-mono">0</span>; choosing another mode removes it, with the password again. A MacBook kept
            awake with its lid closed in a bag can get hot: take it out, or leave it on a desk.
            {power.keep_awake === "lid_closed" && !power.sudoers_rule && " The rule is missing: until it is back, the lid closed sleeps the Mac."}
          </p>
          <Field label="Give sleep back on battery below" hint="lid closed mode; back on above it or plugged in">
            <div className="flex items-center gap-2 text-[12.5px] text-muted">
              <NumberField
                value={power.min_battery_percent}
                min={5}
                max={95}
                busy={busy}
                onSave={(v) => update({ power: { min_battery_percent: v } })}
                testId="keep-awake-min-battery"
              />
              <span>%</span>
            </div>
          </Field>
        </>
      )}
    </>
  );
}

function Linear({ settings, update, busy }: SectionProps) {
  const { enabled, start_tickets, key_saved, key_from_env } = settings.linear;
  return (
    <>
      <Heading title="Linear" />
      <Field label="Linear API key" hint="Linear → Settings → Security & access → API keys">
        <KeyField
          saved={key_saved}
          busy={busy}
          onSave={(key) => update({ linear: { api_key: key } })}
          testId="linear-key"
        />
      </Field>
      {key_from_env && (
        <p className="text-[11.5px] text-attention">CHAUFFEUR_LINEAR_API_KEY is set for the daemon: it is used instead.</p>
      )}
      <Switch
        label="Use Linear"
        hint="Pick tickets when you create a workspace and brief the agent with them. Off: tickets are attached by identifier only."
        on={enabled}
        disabled={busy}
        onChange={(on) => void update({ linear: { enabled: on } })}
        testId="linear-enabled"
      />
      <Switch
        label="Move tickets to In Progress"
        hint="When a workspace starts on them (explorations never move them)."
        on={start_tickets}
        disabled={busy || !enabled}
        onChange={(on) => void update({ linear: { start_tickets: on } })}
        testId="linear-start-tickets"
      />
    </>
  );
}

function Compaction({ settings, update, busy }: SectionProps) {
  const jev = settings.jev;
  const [threshold, setThreshold] = useState(String(jev.threshold_percent));
  useEffect(() => setThreshold(String(jev.threshold_percent)), [jev.threshold_percent]);
  const saveThreshold = () => {
    const percent = Number(threshold);
    if (percent === jev.threshold_percent) return;
    if (!Number.isInteger(percent) || percent < 1 || percent > 95) {
      setThreshold(String(jev.threshold_percent));
      return;
    }
    void update({ jev: { threshold_percent: percent } });
  };
  return (
    <>
      <Heading title="Compaction of chats">
        When a chat's context fills up, Claude Code replaces the conversation with a summary, and Codex keeps only your
        messages and a summary. Jev (TypeSafe) instead keeps the conversation word for word and drops only the tool
        calls and outputs no longer needed. Type /compact to compact a chat now.
      </Heading>
      <Switch
        label="Compact with Jev"
        hint={jev.key_saved ? "Applies from each chat's next message." : "Save a TypeSafe API key first."}
        on={jev.enabled}
        disabled={busy || (!jev.enabled && !jev.key_saved)}
        onChange={(on) => void update({ jev: { enabled: on } })}
        testId="jev-switch"
      />
      <Field label="TypeSafe API key" hint={`kept in ${secretStore(false)}`}>
        <KeyField saved={jev.key_saved} busy={busy} onSave={(key) => update({ jev: { api_key: key } })} testId="jev-key" />
      </Field>
      <Field label="Compact from" hint="as the context meter counts it">
        <div className="flex items-center gap-2">
          <span className="w-16 shrink-0">
            <input
              type="number"
              min={1}
              max={95}
              value={threshold}
              onChange={(event) => setThreshold(event.target.value)}
              onBlur={saveThreshold}
              onKeyDown={(event) => {
                if (event.key === "Enter") saveThreshold();
              }}
              className={inputClass}
              data-testid="jev-threshold"
            />
          </span>
          <span className="text-[12.5px] text-muted">% of the context, checked after each turn; never twice in a row</span>
        </div>
      </Field>
      <p className="text-[11.5px] leading-relaxed text-faint">
        While Jev is on, compacting sends the conversation (tool outputs abridged) to TypeSafe. If Jev fails or would
        remove too little, the agent compacts its own way. A Codex chat compacted by Jev goes on in a new Codex thread.
      </p>
    </>
  );
}

/** Fonts of the interface, the code and the terminal, applied as they are saved. */
function AppearanceSection({ settings, update, busy }: SectionProps) {
  const a = settings.appearance;
  return (
    <>
      <Heading title="Fonts">
        Any font installed on this computer, by name; one that is not installed falls back to the app's own. Also in
        the [appearance] table of settings.toml. The interface's size is the app's zoom ({kbd("⌘+")} / {kbd("⌘−")}).
      </Heading>
      <FontRow
        label="Interface"
        family={a.interface_font}
        fallback={DEFAULT_SANS}
        suggestions={SANS_SUGGESTIONS}
        busy={busy}
        onSave={(family) => update({ appearance: { interface_font: family } })}
        testId="font-interface"
      />
      <FontRow
        label="Code"
        hint="the Files editor, the Diff and code in chats"
        family={a.code_font}
        size={a.code_size || DEFAULT_CODE_SIZE}
        fallback={DEFAULT_MONO}
        suggestions={MONO_SUGGESTIONS}
        busy={busy}
        onSave={(family) => update({ appearance: { code_font: family } })}
        onSize={(size) => update({ appearance: { code_size: size === DEFAULT_CODE_SIZE ? 0 : size } })}
        testId="font-code"
      />
      <FontRow
        label="Terminal"
        family={a.terminal_font}
        size={a.terminal_size || DEFAULT_TERMINAL_SIZE}
        fallback={DEFAULT_MONO}
        suggestions={MONO_SUGGESTIONS}
        busy={busy}
        onSave={(family) => update({ appearance: { terminal_font: family } })}
        onSize={(size) => update({ appearance: { terminal_size: size === DEFAULT_TERMINAL_SIZE ? 0 : size } })}
        testId="font-terminal"
      />
      <Heading title="Colours">
        A terminal theme file, as your tools write it: pywal (colors.json), kitty, Ghostty, foot or Alacritty — Dracula and
        most themes ship one, and wallust writes any of them from a template. The terminal takes its colours, the code its 16 ANSI colours. Read again when it
        changes; also terminal_theme in the [appearance] table of settings.toml.
      </Heading>
      <ThemeField theme={a.terminal_theme} palette={a.palette} error={a.palette_error} busy={busy} onSave={(path) => update({ appearance: { terminal_theme: path } })} />
      <Switch
        label="Also colour the interface"
        hint={interfaceHint(a.terminal_theme, a.palette)}
        on={a.theme_interface}
        disabled={busy || !a.terminal_theme}
        onChange={(on) => void update({ appearance: { theme_interface: on } })}
        testId="theme-interface"
      />
    </>
  );
}

function ThemeField({
  theme,
  palette,
  error,
  busy,
  onSave,
}: {
  theme: string;
  palette: Palette | null;
  error: string | null;
  busy: boolean;
  onSave: (path: string) => Promise<boolean>;
}) {
  const [value, setValue] = useState(theme);
  useEffect(() => setValue(theme), [theme]);
  const save = () => {
    if (value.trim() !== theme) void onSave(value.trim());
  };
  return (
    <Field label="Terminal theme" hint="default: the app's own colours">
      <input
        value={value}
        placeholder="~/.cache/wal/colors.json"
        onChange={(event) => setValue(event.target.value)}
        onBlur={save}
        onKeyDown={(event) => {
          if (event.key === "Enter") save();
        }}
        disabled={busy}
        className={inputClass}
        data-testid="terminal-theme"
      />
      {palette && (
        <div
          className="mt-1.5 flex items-center gap-2 rounded-md border border-line px-2.5 py-1.5 font-mono text-[12px]"
          style={{ background: palette.background, color: palette.foreground }}
          data-testid="palette-preview"
        >
          <span className="shrink-0">$ ls</span>
          <span className="flex flex-wrap gap-1">
            {palette.ansi.map((colour, i) => (
              <span key={i} title={`color${i} ${colour}`} className="h-3.5 w-3.5 rounded-sm" style={{ background: colour }} />
            ))}
          </span>
        </div>
      )}
      {error && <p className="mt-1 text-[11.5px] text-danger selectable">{error}</p>}
    </Field>
  );
}

function FontRow({
  label,
  hint,
  family,
  size,
  fallback,
  suggestions,
  busy,
  onSave,
  onSize,
  testId,
}: {
  label: string;
  hint?: string;
  family: string;
  size?: number;
  fallback: string;
  suggestions: string[];
  busy: boolean;
  onSave: (family: string) => Promise<boolean>;
  onSize?: (size: number) => Promise<boolean>;
  testId: string;
}) {
  const [value, setValue] = useState(family);
  const [sizeValue, setSizeValue] = useState(String(size ?? ""));
  useEffect(() => setValue(family), [family]);
  useEffect(() => setSizeValue(String(size ?? "")), [size]);
  const save = () => {
    if (value.trim() !== family) void onSave(value.trim());
  };
  const saveSize = () => {
    const next = Number(sizeValue);
    if (!onSize || next === size) return;
    if (!Number.isInteger(next) || next < 8 || next > 32) return setSizeValue(String(size ?? ""));
    void onSize(next);
  };
  const list = `${testId}-suggestions`;
  return (
    <Field label={label} hint={hint ?? "default: the app's own"}>
      <div className="flex items-center gap-2">
        <input
          value={value}
          list={list}
          // Apple's fonts lead the app's own: elsewhere the generic family at its end is what shows.
          placeholder={fallback.split(",").at(isMac ? 0 : -1)?.trim().replaceAll('"', "")}
          onChange={(event) => setValue(event.target.value)}
          onBlur={save}
          onKeyDown={(event) => {
            if (event.key === "Enter") save();
          }}
          disabled={busy}
          className={inputClass}
          data-testid={testId}
        />
        <datalist id={list}>
          {suggestions.map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
        {onSize && (
          <span className="flex w-20 shrink-0 items-center gap-1">
            <input
              type="number"
              min={8}
              max={32}
              value={sizeValue}
              onChange={(event) => setSizeValue(event.target.value)}
              onBlur={saveSize}
              onKeyDown={(event) => {
                if (event.key === "Enter") saveSize();
              }}
              disabled={busy}
              className={inputClass}
              aria-label={`${label} size`}
              data-testid={`${testId}-size`}
            />
            <span className="text-[12px] text-faint">px</span>
          </span>
        )}
      </div>
      <div
        className="mt-1.5 truncate rounded-md border border-line bg-bg px-2.5 py-1.5 text-fg"
        style={{ fontFamily: fontStack(value, fallback), fontSize: size ? `${size}px` : undefined }}
      >
        The quick brown fox → {"{ id: 42, ok: true }"} 0O il1
      </div>
    </Field>
  );
}

/** Vim in the Files editor, and the shortcuts' keys: click one's keys, then press the new ones. */
function KeyboardSection({ settings, update, busy }: SectionProps) {
  const { bindings = {}, binding_errors = [] } = settings.keyboard;
  const member = useApp((s) => s.identity?.member ?? false);
  // The shortcut whose keys are being recorded, the keys held so far, and why the last ones were refused.
  const [recording, setRecording] = useState<{ id: ShortcutId; typed?: string; refused?: string } | null>(null);
  const recordingId = recording?.id ?? null;
  // The macOS menu would take its keys (⌘W, ⌘,…) before the recorder.
  const suspendMenu = recordingId !== null;
  useEffect(() => {
    if (!suspendMenu) return;
    setMenuKeys({ recording: true });
    return () => setMenuKeys({ recording: false });
  }, [suspendMenu]);
  useEffect(() => {
    if (!recordingId) return;
    const cancel = () => setRecording(null);
    // Before every other listener: the keys pressed are the new binding, not a shortcut.
    const onKey = (event: KeyboardEvent) => {
      // Tab moves the focus on, as anywhere else, and so ends the recording.
      if (event.key === "Tab" && !event.metaKey && !event.ctrlKey) return cancel();
      event.preventDefault();
      event.stopImmediatePropagation();
      const bare = !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey;
      if (event.key === "Escape" && bare) return cancel();
      if (event.key === "Backspace" && bare) {
        cancel();
        const next = { ...bindings };
        delete next[recordingId];
        if (bindings[recordingId] !== undefined) void update({ keyboard: { bindings: next } });
        return;
      }
      const { typed, keys, refused } = recordBinding(event);
      if (!keys) return setRecording({ id: recordingId, typed, refused });
      cancel();
      void update({ keyboard: { bindings: rebind(bindings, recordingId, keys) } });
    };
    // Letting a modifier go shows the ones still held.
    const onKeyUp = (event: KeyboardEvent) => {
      const { typed, keys, refused } = recordBinding(event);
      if (keys || refused) return;
      setRecording((current) => current && { ...current, typed: typed === "…" ? undefined : typed });
    };
    // A click elsewhere, or the window left, ends it; a click on its own keys toggles it (onClick).
    const onPointer = (event: PointerEvent) => {
      if (!(event.target as Element | null)?.closest?.("[data-recording]")) cancel();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("keyup", onKeyUp, true);
    window.addEventListener("pointerdown", onPointer, true);
    window.addEventListener("blur", cancel);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("pointerdown", onPointer, true);
      window.removeEventListener("blur", cancel);
    };
  }, [recordingId, bindings, update]);
  const changed = Object.keys(bindings).length > 0 || binding_errors.length > 0;
  const reserved = Object.keys(RESERVED)
    .filter((keys) => isMac || !keys.startsWith("⌃"))
    .map((keys) => kbd(keys))
    .join(", ");
  return (
    <>
      <Switch
        label="Vim mode in the Files editor"
        hint="Normal, insert and visual modes; :w saves, Esc stays in the editor."
        on={settings.keyboard.vim_mode}
        disabled={busy}
        onChange={(on) => void update({ keyboard: { vim_mode: on } })}
        testId="vim-mode"
      />
      <div className="flex items-start justify-between gap-3">
        <Heading title="Shortcuts">
          Click a shortcut&apos;s keys, then press the new ones: Esc cancels, ⌫ goes back to the default. Kept in [keyboard.bindings] of
          settings.toml. {reserved} stay the system&apos;s and the terminal&apos;s.
          {!isMac &&
            " In the terminal, the default Ctrl keys stay the shell's (Ctrl+T, Ctrl+W…), except those that switch tabs or views and open or close terminals; keys you set are the app's there too."}
        </Heading>
        <Button size="sm" disabled={busy || !changed} onClick={() => void update({ keyboard: { bindings: {} } })} data-testid="shortcuts-reset">
          Reset all
        </Button>
      </div>
      {binding_errors.map((error) => (
        <p key={error} className="text-[12px] text-danger selectable">
          {error}
        </p>
      ))}
      <div className="divide-y divide-line rounded-md border border-line bg-raised">
        {SHORTCUT_IDS.filter((id) => keysOf(id, isMac, {}).length > 0 && (member || id !== "office.show")).map((id) => {
          const shortcut = SHORTCUTS[id];
          const own = shortcut.fixed ? undefined : bindings[id as ShortcutId];
          const conflicts = conflictsOf(id, isMac, bindings);
          const mine = recording?.id === id ? recording : null;
          return (
            <div key={id} className="flex items-center justify-between gap-3 px-3 py-1.5" data-testid={`shortcut-${id}`}>
              <span className="min-w-0">
                <span className="block text-[12.5px] text-fg">{shortcut.label}</span>
                <span className="block text-[11px] text-faint">
                  {shortcut.where}
                  {own !== undefined && ` · default ${labelOf(id, isMac, {})}`}
                </span>
                {conflicts.length > 0 && (
                  <span className="block text-[11px] text-attention">Same keys as {conflicts.map((other) => SHORTCUTS[other].label).join(", ")}</span>
                )}
                {mine?.refused && (
                  <span className="block text-[11px] text-danger" data-testid={`shortcut-refused-${id}`}>
                    {mine.refused}
                  </span>
                )}
              </span>
              {shortcut.fixed ? (
                <span className="shrink-0 text-[11.5px] text-faint">{labelOf(id)}</span>
              ) : (
                <button
                  type="button"
                  disabled={busy}
                  onClick={(event) => {
                    // WebKit leaves a clicked button without the focus; Tab goes on from here.
                    event.currentTarget.focus();
                    setRecording(mine ? null : { id: id as ShortcutId });
                  }}
                  data-popover={mine ? "" : undefined}
                  data-recording={mine ? "" : undefined}
                  className={cx(
                    "shrink-0 rounded border px-1.5 py-0.5 text-[11.5px]",
                    mine ? "border-accent text-fg" : own !== undefined ? "border-line-strong text-fg" : "border-line text-muted hover:text-fg",
                  )}
                  data-testid={`shortcut-keys-${id}`}
                >
                  {mine ? (mine.typed ?? "Press keys…") : labelOf(id, isMac, bindings)}
                </button>
              )}
            </div>
          );
        })}
      </div>
    </>
  );
}

/** The sound of a chat that finishes or needs you: none, one of the app's own, or your file. */
function Notifications({ settings, update, busy }: SectionProps) {
  const { sound, sound_error } = settings.notifications;
  const custom = sound !== "" && !BUILT_IN_SOUNDS[sound];
  const [choice, setChoice] = useState(custom ? "file" : sound || "none");
  const [path, setPath] = useState(custom ? sound : "");
  const [playError, setPlayError] = useState<string | null>(null);
  useEffect(() => {
    const isCustom = sound !== "" && !BUILT_IN_SOUNDS[sound];
    setChoice(isCustom ? "file" : sound || "none");
    if (isCustom) setPath(sound);
  }, [sound]);
  const pick = (next: string) => {
    setChoice(next);
    setPlayError(null);
    if (next !== "file") void update({ notifications: { sound: next === "none" ? "" : next } }).then((ok) => ok && next !== "none" && void playNotificationSound(next));
  };
  const savePath = () => {
    if (path.trim() && path.trim() !== sound) void update({ notifications: { sound: path.trim() } });
  };
  const play = async () => {
    setPlayError(null);
    try {
      await playNotificationSound(sound);
    } catch (err) {
      setPlayError(errorMessage(err));
    }
  };
  return (
    <>
      <Heading title="Sound">
        Played with the notification when a chat finishes, fails or needs you while Chauffeur is not in front. Also
        sound in the [notifications] table of settings.toml.
      </Heading>
      <Field label="Sound" hint="the app's own sounds are made by Chauffeur">
        <div className="flex items-center gap-2">
          <select value={choice} onChange={(event) => pick(event.target.value)} disabled={busy} className={inputClass} data-testid="sound-choice">
            <option value="none">None</option>
            {Object.entries(BUILT_IN_SOUNDS).map(([name, { label }]) => (
              <option key={name} value={name}>
                {label}
              </option>
            ))}
            <option value="file">A file of yours…</option>
          </select>
          <Button size="sm" onClick={() => void play()} disabled={busy || !sound} title="Play it" data-testid="sound-play">
            <Play size={12} /> Play
          </Button>
        </div>
      </Field>
      {choice === "file" && (
        <Field label="Sound file" hint="MP3, WAV, AIFF, M4A, OGG or FLAC, at most 5 MB">
          <input
            value={path}
            placeholder="~/Music/jingle.mp3"
            onChange={(event) => setPath(event.target.value)}
            onBlur={savePath}
            onKeyDown={(event) => {
              if (event.key === "Enter") savePath();
            }}
            disabled={busy}
            className={inputClass}
            data-testid="sound-file"
          />
        </Field>
      )}
      {(sound_error || playError) && <p className="text-[12px] text-danger selectable">{sound_error ?? playError}</p>}
    </>
  );
}

/** The version of this app is the one its UI was built with (the release bumps it with the shell's). */
function About() {
  const connection = useApp((s) => s.connection);
  const daemonVersion = connection.status === "open" ? connection.daemonVersion : undefined;
  const [update, setUpdate] = useState<UpdateStatus | null>(null);
  useEffect(() => {
    client
      .call("update.status", {})
      .then(setUpdate)
      .catch(() => {});
  }, []);
  const mismatch = daemonMismatch(APP_VERSION, daemonVersion);
  const newer = updateNote(update);
  return (
    <div className="flex flex-col items-center gap-3 pt-6 text-center" data-testid="about">
      <img src={appIcon} alt="" width={72} height={72} draggable={false} />
      <div>
        <div className="text-[15px] font-semibold text-fg">Chauffeur</div>
        <p className="mt-1 text-[12px] text-muted">Run Claude Code and Codex on a ticket that spans several repos.</p>
      </div>
      <dl className="grid grid-cols-[auto_auto] gap-x-3 gap-y-0.5 text-[12px] selectable">
        <dt className="text-right text-muted">Version</dt>
        <dd className="text-left font-mono text-fg" data-testid="about-app-version">
          {APP_VERSION}
        </dd>
        <dt className="text-right text-muted">Daemon</dt>
        <dd className="text-left font-mono text-fg" data-testid="about-daemon-version">
          {daemonVersion ?? "not connected"}
        </dd>
      </dl>
      {mismatch && <p className="max-w-sm text-[11.5px] text-attention selectable">{mismatch}</p>}
      {newer && <p className="max-w-sm text-[11.5px] text-muted">{newer}</p>}
      {update?.releases_url && (
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            const url = update.releases_url;
            if (url) void openExternal(url).catch((err) => toast(`Could not open the browser: ${errorMessage(err)}`, "error"));
          }}
        >
          <ExternalLink size={12} /> Release notes
        </Button>
      )}
      <p className="text-[11px] text-faint">By Dialog</p>
    </div>
  );
}

function SearchSection({ settings, update, busy }: SectionProps) {
  const [status, setStatus] = useState<SearchStatus | null>(null);
  const [pull, setPull] = useState<OllamaPull | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [model, setModel] = useState(settings.search.embedding_model);
  useEffect(() => setModel(settings.search.embedding_model), [settings.search.embedding_model]);
  useEffect(() => {
    let stopped = false;
    const refresh = async () => {
      try {
        const [search, ollama] = await Promise.all([client.call("search.status", {}), client.call("ollama.status", {})]);
        if (stopped) return;
        setStatus(search);
        setPull(ollama.pulls.find((p) => p.model === search.model) ?? null);
      } catch (err) {
        if (!stopped) setError(errorMessage(err));
      }
    };
    void refresh();
    // The indexing's and a download's progress come by asking again, as an update's does.
    const timer = setInterval(() => void refresh(), 2000);
    return () => { stopped = true; clearInterval(timer); };
  }, []);
  const saveModel = () => {
    if (model.trim() !== settings.search.embedding_model) void update({ search: { embedding_model: model.trim() } });
  };
  const download = async () => {
    if (!status) return;
    setError(null);
    try {
      await client.call("ollama.pull", { model: status.model });
    } catch (err) {
      setError(`Could not download ${status.model}: ${errorMessage(err)}`);
    }
  };
  const line = status && semanticLine(status);
  return (
    <>
      <Heading title="Search">
        {kbd("⌘K")} searches every workspace, active and archived: their titles, tickets and pull requests, their chats' messages and their notes. By keywords always, and by meaning through a small model Ollama runs on this machine: nothing leaves it.
      </Heading>
      <Switch
        label="Search by meaning"
        hint="Finds a message by what it says, in French or English, even without its words. The history is indexed in the background, on the charger."
        on={settings.search.semantic}
        disabled={busy}
        onChange={(on) => void update({ search: { semantic: on } })}
        testId="search-semantic"
      />
      <Field label="Embedding model" hint="embeddinggemma by default; another one indexes everything again">
        <input
          className={inputClass}
          value={model}
          placeholder="embeddinggemma"
          onChange={(e) => setModel(e.target.value)}
          onBlur={saveModel}
          onKeyDown={(e) => e.key === "Enter" && saveModel()}
          disabled={busy || !settings.search.semantic}
          data-testid="search-embedding-model"
        />
      </Field>
      {status && (
        <div className="space-y-1.5 rounded-md border border-line bg-raised px-3 py-2 text-[12px]" data-testid="search-status">
          <div className="text-fg">
            {status.total === 0 ? "Nothing indexed yet." : `${status.embedded.toLocaleString()} of ${status.total.toLocaleString()} pieces of text indexed by meaning.`}
          </div>
          {line && <div className="text-muted">{line.text}</div>}
          {pull && <div className="text-muted">{pullLabel(pull)}</div>}
          {line?.action === "download" && !(pull && !pull.error) && (
            <Button size="sm" onClick={() => void download()} data-testid="search-download-model">Download {status.model}</Button>
          )}
        </div>
      )}
      {error && <p className="text-[12px] text-danger selectable">{error}</p>}
    </>
  );
}

function Heading({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div>
      <div className="mb-1 font-medium text-fg">{title}</div>
      {children && <p className="text-[12px] leading-relaxed text-muted">{children}</p>}
    </div>
  );
}

function Switch({
  label,
  hint,
  on,
  disabled,
  onChange,
  testId,
  icon,
}: {
  label: string;
  hint: string;
  on: boolean;
  disabled?: boolean;
  onChange: (on: boolean) => void;
  testId: string;
  icon?: ReactNode;
}) {
  return (
    <button
      role="switch"
      aria-checked={on}
      disabled={disabled}
      onClick={() => onChange(!on)}
      className="flex w-full items-center justify-between gap-3 rounded-md border border-line bg-raised px-3 py-2 text-left disabled:opacity-60"
      data-testid={testId}
    >
      <span className="min-w-0">
        <span className="flex items-center gap-1.5 text-[12.5px] text-fg">
          {icon}
          {label}
        </span>
        <span className="block text-[11.5px] leading-snug text-faint">{hint}</span>
      </span>
      <Toggle on={on} />
    </button>
  );
}

/** A saved secret shows as dots, with Replace and Remove; otherwise a field to paste one. */
function KeyField({
  saved,
  busy,
  onSave,
  testId,
}: {
  saved: boolean;
  busy: boolean;
  onSave: (key: string) => Promise<boolean>;
  testId: string;
}) {
  const [key, setKey] = useState("");
  const [editing, setEditing] = useState(false);
  const save = async () => {
    if (key.trim() && (await onSave(key.trim()))) {
      setKey("");
      setEditing(false);
    }
  };
  if (saved && !editing) {
    return (
      <div className="flex items-center gap-2">
        <span className={cx(inputClass, "flex items-center text-faint")}>•••••••••••• saved</span>
        <Button size="sm" onClick={() => setEditing(true)} disabled={busy}>
          Replace
        </Button>
        <Button size="sm" variant="danger" onClick={() => void onSave("")} disabled={busy} data-testid={`${testId}-remove`}>
          Remove
        </Button>
      </div>
    );
  }
  return (
    <form
      className="flex items-center gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <input
        type="password"
        autoFocus={editing}
        value={key}
        onChange={(event) => setKey(event.target.value)}
        placeholder="Paste the key"
        autoComplete="off"
        spellCheck={false}
        className={inputClass}
        data-testid={testId}
      />
      <Button size="sm" variant="primary" type="submit" disabled={!key.trim()} busy={busy}>
        Save
      </Button>
      {editing && (
        <Button size="sm" variant="ghost" type="button" onClick={() => setEditing(false)}>
          Cancel
        </Button>
      )}
    </form>
  );
}

/** Chips with a remove button, and a field (with suggestions) to add one. */
function ListEditor({
  values,
  suggestions = [],
  placeholder,
  disabled,
  mono = false,
  onChange,
  testId,
}: {
  values: string[];
  suggestions?: string[];
  placeholder: string;
  disabled: boolean;
  mono?: boolean;
  onChange: (values: string[]) => void;
  testId: string;
}) {
  const [draft, setDraft] = useState("");
  const listId = `${testId}-suggestions`;
  const add = () => {
    const value = draft.trim();
    if (!value) return;
    setDraft("");
    if (!values.includes(value)) onChange([...values, value]);
  };
  return (
    <div className="space-y-1.5" data-testid={testId}>
      <div className="flex flex-wrap gap-1.5">
        {values.length === 0 && <span className="text-[12px] text-faint">None</span>}
        {values.map((value) => (
          <span
            key={value}
            className={cx(
              "inline-flex items-center gap-1 rounded-md border border-line bg-raised py-0.5 pr-1 pl-2 text-[12px] text-fg",
              mono && "font-mono",
            )}
          >
            {value}
            <button
              title={`Remove ${value}`}
              aria-label={`Remove ${value}`}
              disabled={disabled}
              onClick={() => onChange(values.filter((v) => v !== value))}
              className="rounded p-0.5 text-faint hover:bg-hover hover:text-fg"
            >
              <X size={11} />
            </button>
          </span>
        ))}
      </div>
      <form
        className="flex items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          add();
        }}
      >
        <input
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          list={suggestions.length > 0 ? listId : undefined}
          placeholder={placeholder}
          spellCheck={false}
          className={cx(inputClass, mono && "font-mono")}
          data-testid={`${testId}-input`}
        />
        {suggestions.length > 0 && (
          <datalist id={listId}>
            {suggestions
              .filter((s) => !values.includes(s))
              .map((s) => (
                <option key={s} value={s} />
              ))}
          </datalist>
        )}
        <Button size="sm" type="submit" disabled={disabled || !draft.trim()}>
          <Plus size={12} /> Add
        </Button>
      </form>
    </div>
  );
}
