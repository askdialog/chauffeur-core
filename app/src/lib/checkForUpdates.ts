// The app menu's Check for Updates…: asks for the newest release now, then either says Chauffeur is
// up to date or brings the sidebar's update card back, even where the user hid that version, so that
// the one step (Update now, or When idle) is offered from the menu too.

import { errorMessage } from "./rpc";
import { DISMISSED_KEY, UPDATE_REVEAL_EVENT, upToDate } from "./update";
import { client, showOffice, toast, toggleSidebar, useApp } from "@/state/app";

export async function checkForUpdates(): Promise<void> {
  try {
    const status = await client.call("update.check", {});
    const message = upToDate(status);
    if (message) return void toast(message);
    // The card lives in the sidebar: bring it back before it is told to show again.
    localStorage.removeItem(DISMISSED_KEY);
    if (useApp.getState().officeShown) showOffice(false);
    if (useApp.getState().sidebarHidden) toggleSidebar(false);
    window.dispatchEvent(new Event(UPDATE_REVEAL_EVENT));
  } catch (error) {
    toast(`Could not check for updates: ${errorMessage(error)}`, "error");
  }
}
