import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../index.css";
import { sharePageOf } from "@/lib/workspaceSharePage";
import { SharePage } from "./SharePage";
import { WorkspaceSharePage } from "./WorkspaceSharePage";

// One build serves both kinds of share: a design's at /s/<id>, a workspace's at /w/<id>.
const page = sharePageOf(location.pathname);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {page?.kind === "workspace" ? <WorkspaceSharePage id={page.id} /> : <SharePage />}
  </StrictMode>,
);
