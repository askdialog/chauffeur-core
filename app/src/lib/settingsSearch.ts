import { MEMBERS_ONLY_SECTIONS } from "./members";

export type SettingsSection = "brands" | "account" | "extensions" | "slack" | "transfer" | "mobile" | "general" | "github" | "daily-review" | "appearance" | "keyboard" | "notifications" | "workspaces" | "llms" | "agents" | "local-models" | "search" | "sleep" | "linear" | "compaction" | "about";

export interface SettingsOption {
  section: SettingsSection;
  label: string;
  target: string;
  keywords?: string;
}

// Targets name stable controls, including the controlling option for conditional fields.
export const SETTINGS_OPTIONS: readonly SettingsOption[] = [
  { section: "general", label: "Start Chauffeur at login", target: "start-at-login", keywords: "startup launch boot"  },
  { section: "general", label: "Daemon port", target: "daemon-port", keywords: "network"  },
  { section: "general", label: "Settings file", target: "open-settings-file", keywords: "settings.toml configuration"  },
  { section: "appearance", label: "Interface font", target: "font-interface", keywords: "typography text zoom"  },
  { section: "appearance", label: "Code font and size", target: "font-code", keywords: "editor typography"  },
  { section: "appearance", label: "Terminal font and size", target: "font-terminal", keywords: "typography"  },
  { section: "appearance", label: "Terminal theme", target: "terminal-theme", keywords: "colors colours palette dark light"  },
  { section: "appearance", label: "Also colour the interface", target: "theme-interface", keywords: "theme colors colours"  },
  { section: "keyboard", label: "Vim mode", target: "vim-mode", keywords: "editor"  },
  { section: "keyboard", label: "Keyboard shortcuts", target: "shortcuts-list", keywords: "key bindings hotkeys"  },
  { section: "notifications", label: "Sound", target: "sound-choice", keywords: "audio chime bell station file notification"  },
  { section: "workspaces", label: "Default repositories", target: "default-repos", keywords: "repos new workspace"  },
  { section: "workspaces", label: "Branch prefix", target: "branch-prefix", keywords: "git"  },
  { section: "workspaces", label: "Folders with your clones", target: "clone-roots", keywords: "repositories directory"  },
  { section: "github", label: "GitLab host", target: "gitlab-host", keywords: "forge gitlab self-hosted merge requests mr"  },
  { section: "workspaces", label: "Main GitHub organization", target: "github-org", keywords: "repos"  },
  { section: "workspaces", label: "GitHub repository sources", target: "github-sources", keywords: "account repos"  },
  { section: "workspaces", label: "Repositories elsewhere", target: "repo-table", keywords: "repos default branch"  },
  { section: "workspaces", label: "Workspace ports", target: "port-range-start", keywords: "port block size"  },
  { section: "workspaces", label: "Archive script timeout", target: "archive-timeout", keywords: "archiving"  },
  { section: "workspaces", label: "Snapshot retention", target: "snapshot-days", keywords: "archiving days"  },
  { section: "llms", label: "Model for chauffeur new", target: "cli-model-switch", keywords: "cli default"  },
  { section: "daily-review", label: "Daily session review", target: "daily-review-enabled", keywords: "schedule 18:00"  },
  { section: "daily-review", label: "Daily review model", target: "daily-review-model-override", keywords: "agent"  },
  { section: "agents", label: "Idle agents", target: "idle-minutes", keywords: "memory timeout"  },
  { section: "agents", label: "Suggest my next message", target: "prompt-suggestions", keywords: "prompt suggestions"  },
  { section: "sleep", label: "Wake for the daily review", target: "daily-review-wake", keywords: "schedule"  },
  { section: "sleep", label: "Keep the machine awake", target: "keep-awake-never", keywords: "battery lid power sleep"  },
  { section: "linear", label: "Linear API key", target: "linear-key", keywords: "integration token"  },
  { section: "linear", label: "Use Linear", target: "linear-enabled", keywords: "integration"  },
  { section: "linear", label: "Move tickets to In Progress", target: "linear-start-tickets", keywords: "issues status"  },
  { section: "compaction", label: "Compact with Jev", target: "jev-switch", keywords: "context typesafe"  },
  { section: "compaction", label: "TypeSafe API key", target: "jev-key", keywords: "jev token"  },
  { section: "compaction", label: "Compaction threshold", target: "jev-threshold", keywords: "context percentage compact from"  },
  { section: "search", label: "Search by meaning", target: "search-semantic", keywords: "semantic ollama"  },
  { section: "search", label: "Embedding model", target: "search-embedding-model", keywords: "ollama embeddinggemma"  },
];

/** All query words must match; member-only options never appear outside a member session. */
export function searchSettings(query: string, member: boolean): SettingsOption[] {
  const words = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  return SETTINGS_OPTIONS.filter((option) =>
    (member || !MEMBERS_ONLY_SECTIONS.includes(option.section)) &&
    words.every((word) => `${option.section} ${option.label} ${option.keywords ?? ""}`.toLocaleLowerCase().includes(word)),
  );
}
