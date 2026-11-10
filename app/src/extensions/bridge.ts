// Core's side of the extension UI SDK (app/sdk/modules.ts): puts the shared modules on the page
// for the separately built `extension.js` files to take, so that they use this app's React,
// CodeMirror, stores and components rather than copies of their own.

import * as codemirror from "codemirror";
import * as commands from "@codemirror/commands";
import * as langCss from "@codemirror/lang-css";
import * as langHtml from "@codemirror/lang-html";
import * as langJavascript from "@codemirror/lang-javascript";
import * as langJson from "@codemirror/lang-json";
import * as langMarkdown from "@codemirror/lang-markdown";
import * as langPython from "@codemirror/lang-python";
import * as langRust from "@codemirror/lang-rust";
import * as langSql from "@codemirror/lang-sql";
import * as langXml from "@codemirror/lang-xml";
import * as langYaml from "@codemirror/lang-yaml";
import * as language from "@codemirror/language";
import * as search from "@codemirror/search";
import * as cmState from "@codemirror/state";
import * as cmView from "@codemirror/view";
import * as highlight from "@lezer/highlight";
import * as protocol from "@chauffeur/protocol";
import * as React from "react";
import * as jsxRuntime from "react/jsx-runtime";
import * as ReactDOM from "react-dom";
import * as ReactDOMClient from "react-dom/client";
import { SDK_GLOBAL } from "../../sdk/modules";
import { makeBridge } from "../lib/sdkBridge";
import * as App from "../App";
import * as ArchiveDialog from "../components/ArchiveDialog";
import * as ErrorBoundary from "../components/ErrorBoundary";
import * as Markdown from "../components/Markdown";
import * as ChatPanel from "../components/chat/ChatPanel";
import * as ui from "../components/ui";
import * as audioHold from "../lib/audioHold";
import * as browserOffice from "../lib/browserOffice";
import * as designs from "../lib/designs";
import * as editorTheme from "../lib/editorTheme";
import * as format from "../lib/format";
import * as keys from "../lib/keys";
import * as models from "../lib/models";
import * as native from "../lib/native";
import * as rpc from "../lib/rpc";
import * as share from "../lib/share";
import * as tabDrag from "../lib/tabDrag";
import * as appState from "../state/app";
import * as session from "../state/session";
import * as sdk from "./sdk";

const namespaces: Record<string, object> = {
  react: React,
  "react/jsx-runtime": jsxRuntime,
  "react-dom": ReactDOM,
  "react-dom/client": ReactDOMClient,
  codemirror,
  "@codemirror/commands": commands,
  "@codemirror/lang-css": langCss,
  "@codemirror/lang-html": langHtml,
  "@codemirror/lang-javascript": langJavascript,
  "@codemirror/lang-json": langJson,
  "@codemirror/lang-markdown": langMarkdown,
  "@codemirror/lang-python": langPython,
  "@codemirror/lang-rust": langRust,
  "@codemirror/lang-sql": langSql,
  "@codemirror/lang-xml": langXml,
  "@codemirror/lang-yaml": langYaml,
  "@codemirror/language": language,
  "@codemirror/search": search,
  "@codemirror/state": cmState,
  "@codemirror/view": cmView,
  "@lezer/highlight": highlight,
  "@chauffeur/protocol": protocol,
  "@chauffeur/core/App": App,
  "@chauffeur/core/components/ArchiveDialog": ArchiveDialog,
  "@chauffeur/core/components/ErrorBoundary": ErrorBoundary,
  "@chauffeur/core/components/Markdown": Markdown,
  "@chauffeur/core/components/chat/ChatPanel": ChatPanel,
  "@chauffeur/core/components/ui": ui,
  "@chauffeur/core/extensions/sdk": sdk,
  "@chauffeur/core/lib/audioHold": audioHold,
  "@chauffeur/core/lib/browserOffice": browserOffice,
  "@chauffeur/core/lib/designs": designs,
  "@chauffeur/core/lib/editorTheme": editorTheme,
  "@chauffeur/core/lib/format": format,
  "@chauffeur/core/lib/keys": keys,
  "@chauffeur/core/lib/models": models,
  "@chauffeur/core/lib/native": native,
  "@chauffeur/core/lib/rpc": rpc,
  "@chauffeur/core/lib/share": share,
  "@chauffeur/core/lib/tabDrag": tabDrag,
  "@chauffeur/core/state/app": appState,
  "@chauffeur/core/state/session": session,
};

/** Once, before any extension loads. */
export function installBridge() {
  const target = globalThis as Record<string, unknown>;
  if (!target[SDK_GLOBAL]) Object.defineProperty(target, SDK_GLOBAL, { value: makeBridge(namespaces) });
}
