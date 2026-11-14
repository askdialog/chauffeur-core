import { lazy, StrictMode, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { BrowserExtensionPage } from "./extensions/browser";
import { setRegistryClient } from "./extensions/registry";
import "./index.css";
import { inBrowserOffice } from "./lib/browserOffice";
import { deliverFileDrop } from "./lib/fileDrops";
import { isTauri, nativeFileDrops, onFileDrop, openExternal, onWorkspaceShareLink } from "./lib/native";
import { errorMessage } from "./lib/rpc";
import { refuseStrayDrop } from "./lib/strayDrops";
import { openStrayLink } from "./lib/strayLinks";
import { client, openCreate, prefillCreate, toast, useApp } from "./state/app";
import { listenForSlackLinks } from "./lib/slackLinks";

// Extensions' interfaces load from their installed packages and use this app's modules (the
// bridge is installed when the first one loads).
setRegistryClient({
  call: (method, params, workspace, id) => client.call("extensions.call", { id, method, params, workspace }),
  watch: (id, onEvent, onState) => client.watchExtension(id, onEvent, onState),
});

// After React's own handlers, which sit on the root: only drops no target accepted get here.
window.addEventListener("dragover", (event) => refuseStrayDrop(event, nativeFileDrops));
window.addEventListener("drop", (event) => refuseStrayDrop(event, nativeFileDrops));
// After React's handlers too: a web link no component opened goes to the browser, a middle click
// too. Only in the app: in a browser (the Office there) a link already opens in a tab.
if (isTauri) {
  const open = (url: string) =>
    void openExternal(url).catch((error) => toast(`Could not open the browser: ${errorMessage(error)}`, "error"));
  window.addEventListener("click", (event) => openStrayLink(event, open));
  window.addEventListener("auxclick", (event) => openStrayLink(event, open));
}
// Linux: files dropped from the desktop reach the drop target under them through the shell.
onFileDrop((drop) => deliverFileDrop(document, drop, useApp.getState().zoom));
void listenForSlackLinks(prefillCreate, (message, error) => toast(message, error ? "error" : "info"))
  .catch((error: unknown) => toast(`Could not receive Slack links: ${errorMessage(error)}`, "error"));

// One listener for the window's lifetime: React StrictMode must not drain a startup link twice.
void onWorkspaceShareLink((link) => openCreate(true, false, null, link))
  .catch((error) => toast(`Could not open the shared workspace: ${errorMessage(error)}`, "error"));

// Development only: `#chat-fixture` renders the chat view with fabricated events,
// `#usage-fixture` the sidebar's plan usage with fabricated readings.
const Fixture = !import.meta.env.DEV
  ? null
  : location.hash === "#mcp-fixture"
    ? lazy(() => import("./dev/ElicitationFixture").then((m) => ({ default: m.ElicitationFixture })))
    : location.hash === "#chat-fixture"
    ? lazy(() => import("./dev/ChatFixture").then((m) => ({ default: m.ChatFixture })))
    : location.hash === "#scroll-fixture"
      ? lazy(() => import("./dev/ScrollFixture").then((m) => ({ default: m.ScrollFixture })))
    : location.hash === "#usage-fixture"
      ? lazy(() => import("./dev/UsageFixture").then((m) => ({ default: m.UsageFixture })))
      : null;
const Page = Fixture ?? (inBrowserOffice ? BrowserExtensionPage : null);


createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {Page ? (
      <Suspense>
        <Page />
      </Suspense>
    ) : (
      <App />
    )}
  </StrictMode>,
);
