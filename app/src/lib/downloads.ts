// Saving a chat's attachments. The desktop app has its shell write them to Downloads (the web view
// cannot save a file, and the daemon's URL holds the app's token, which must not reach a
// browser). A plain browser has no way: its `download` link is ignored across origins and the
// daemon allows no cross-origin fetch, so nothing is offered there.

import { isTauri } from "./native";

export type Download = { url: string; name: string };

/** The folder a set of files is saved in, so that they stay together. */
export const DOWNLOAD_FOLDER = "Chauffeur attachments";

/** Only the desktop app can save a file. */
export const canDownload = isTauri;

/** Saves the files and returns where they went: the file, or the folder of several. */
export async function downloadFiles(files: Download[]): Promise<string> {
  const { invoke } = await import("@tauri-apps/api/core");
  return await invoke<string>("save_downloads", { files, folder: files.length > 1 ? DOWNLOAD_FOLDER : null });
}

/** What the toast says once the files are saved. */
export function savedMessage(count: number, path: string): string {
  return `${count === 1 ? "Saved" : `Saved ${count} files`} to ${path}`;
}
