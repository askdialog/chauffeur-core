import type { ExtensionId, ExtensionInfo } from "@chauffeur/protocol";
import type { SplitLayout, ViewId } from "./splitLayout";

/**
 * Where each official extension's UI goes. Stable contribution IDs also identify saved pane
 * layouts and keyboard commands; the components themselves come from the installed package.
 */
export const EXTENSION_VIEWS = [
  { extension: "database", id: "database", label: "Database", placement: "workspace" },
  { extension: "office", id: "office", label: "Office", placement: "global" },
] as const;

export function extensionOfView(view: string): ExtensionId | undefined {
  return EXTENSION_VIEWS.find((entry) => entry.id === view)?.extension;
}

/**
 * The user wants the extension: enabled, and installed or being installed. A package that is
 * only starting, downloading, missing its files for a moment or failed still counts: its views
 * stay where the user put them and say what is going on, so a saved layout survives an update.
 */
export function extensionEnabled(extensions: ExtensionInfo[] | null, id: ExtensionId): boolean {
  return extensions?.some((extension) => extension.id === id && extension.enabled && extension.package?.desired_installed === true) ?? false;
}

export function viewAvailable(extensions: ExtensionInfo[] | null, view: string): boolean {
  const owner = extensionOfView(view);
  return owner === undefined || extensionEnabled(extensions, owner);
}

/**
 * Disabled or removed contributions cannot reappear through a saved split or its reopen
 * shortcut. Before the catalogue arrives nothing is known yet: the saved layout stays as it is.
 */
export function availableLayout(extensions: ExtensionInfo[] | null, layout: SplitLayout): SplitLayout {
  if (extensions === null) return layout;
  const side = layout.side && viewAvailable(extensions, layout.side) ? layout.side : null;
  const last: ViewId = viewAvailable(extensions, layout.last) ? layout.last : "terminal";
  return side === layout.side && last === layout.last ? layout : { ...layout, side, last };
}

export type PackageStatus = {
  label: string;
  tone: "neutral" | "accent" | "working" | "attention" | "danger" | "success";
  /** One sentence on what is going on, shown under the extension. */
  detail: string | null;
  /** An operation runs: the buttons that would start another one wait. */
  busy: boolean;
};

/** How the Marketplace shows an extension's package. */
export function packageStatus(extension: ExtensionInfo): PackageStatus {
  const pkg = extension.package;
  if (!pkg) return { label: "Built in", tone: "neutral", detail: null, busy: false };
  const update = pkg.available ? `Version ${pkg.available}` : "A new version";
  if (!pkg.desired_installed) {
    return pkg.phase === "downloading"
      ? { label: "Removing", tone: "working", detail: "Cancelling the download in progress.", busy: true }
      : { label: "Not installed", tone: "neutral", detail: pkg.error, busy: false };
  }
  switch (pkg.phase) {
    case "downloading":
      return { label: "Downloading", tone: "working", detail: `Downloading and checking ${pkg.available ? `version ${pkg.available}` : "the extension"}…`, busy: true };
    case "starting":
      return { label: "Starting", tone: "working", detail: "Starting the new version; its views open once it is ready.", busy: true };
    case "waiting":
      // An error beside a staged version: a later check or download failed, the staged one stands.
      return pkg.error
        ? { label: "Update ready", tone: "attention", detail: pkg.error, busy: false }
        : { label: "Update ready", tone: "accent", detail: `${update} is downloaded. It applies once this extension's views are closed, or at the next start.`, busy: false };
    case "failed":
      return { label: "Failed", tone: "danger", detail: pkg.error ?? "The extension could not start.", busy: false };
    case "missing":
      return { label: "Not installed", tone: "attention", detail: pkg.error ?? "Its package is not on this computer yet.", busy: false };
    case "installed":
      if (pkg.error) return { label: "Installed", tone: "attention", detail: pkg.error, busy: false };
      if (pkg.available) return { label: "Update available", tone: "accent", detail: `${update} is available.`, busy: false };
      return { label: "Installed", tone: "success", detail: null, busy: false };
  }
}
