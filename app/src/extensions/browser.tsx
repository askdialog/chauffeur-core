// The Office in the user's browser (lib/browserOffice.ts): the page asks its own listener which
// Office revision is installed (`GET /extension-ui/office`, with the page's session), loads that
// revision's interface and shows its browser page. An Office subscription (`office: true`) is
// taken first and held for the page's life: while it is, the daemon keeps that revision. After
// the connection comes back (the engine may have been updated meanwhile) the metadata is read
// again and the page moves to the new revision.

import { useEffect, useState } from "react";
import { browserConnection, fetchBrowserUi, SESSION_ENDED, SessionEnded } from "@/lib/browserOffice";
import { errorMessage } from "@/lib/rpc";
import { client } from "@/state/app";
import { EmptyState, Spinner } from "@/components/ui";
import { loadExtensionUi, useExtensionUi } from "./registry";

const RETRY_MS = 2000;

export function BrowserExtensionPage() {
  const ui = useExtensionUi("office");
  const [revision, setRevision] = useState<string | null>(null);
  const [problem, setProblem] = useState<{ message: string; final: boolean } | null>(null);
  const [connection, setConnection] = useState(0);

  // The pin: an Office subscription for the whole page.
  useEffect(() => {
    const subscription = client.subscribe({ all: false, registry: false, workspaces: [], chats: [], ephemeral: false, office: true }, () => {});
    let open = client.state.status === "open";
    const stop = client.onStateChange((state) => {
      const now = state.status === "open";
      if (now && !open) setConnection((n) => n + 1);
      open = now;
    });
    client.start();
    return () => {
      stop();
      subscription.close();
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const attempt = async () => {
      try {
        const { token } = await browserConnection();
        const metadata = await fetchBrowserUi(token);
        if (cancelled) return;
        if (!metadata) {
          setProblem({ message: "The Office is starting…", final: false });
          timer = setTimeout(() => void attempt(), RETRY_MS);
          return;
        }
        await loadExtensionUi("office", metadata);
        if (cancelled) return;
        setProblem(null);
        setRevision(metadata.revision);
      } catch (error) {
        if (cancelled) return;
        const final = error instanceof SessionEnded || errorMessage(error) === SESSION_ENDED;
        setProblem({ message: errorMessage(error), final });
        if (!final) timer = setTimeout(() => void attempt(), RETRY_MS);
      }
    };
    void attempt();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [connection]);

  const Page = revision && ui.loaded?.revision === revision ? ui.loaded.contribution.browser : undefined;
  if (Page) return <Page key={revision} />;
  if (problem?.final) return <EmptyState title="The Office is not available">{problem.message}</EmptyState>;
  return (
    <div className="h-full">
      <EmptyState icon={<Spinner />} title={problem?.message ?? "Opening the Office…"} />
    </div>
  );
}
