import { useEffect, useState } from "react";
import type { SlackStatus } from "@chauffeur/protocol";
import { Check, MessageSquare } from "lucide-react";
import { errorMessage } from "@/lib/rpc";
import { openExternal } from "@/lib/native";
import { client } from "@/state/app";
import { Button, Spinner } from "./ui";

export function SlackSettings() {
  const [status, setStatus] = useState<SlackStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const refresh = async (poll: boolean) => {
      try {
        const next = await client.call(poll ? "slack.poll" : "slack.status", {});
        if (!active) return;
        setStatus(next);
        if (next.pending) timer = setTimeout(() => void refresh(true), 2000);
      } catch (err) { if (active) { setError(errorMessage(err)); setStatus(previous => previous ? { ...previous, pending: false } : previous); } }
    };
    void refresh(false);
    return () => { active = false; clearTimeout(timer); };
  }, [revision]);

  async function act(action: "login" | "cancel" | "logout") {
    setBusy(true); setError(null);
    try {
      if (action === "login") {
        const { url } = await client.call("slack.login", {});
        try { await openExternal(url); }
        catch (err) { await client.call("slack.cancel", {}); throw err; }
      } else if (action === "cancel") await client.call("slack.cancel", {});
      else await client.call("slack.logout", {});
    } catch (err) { setError(errorMessage(err)); }
    finally { setBusy(false); setRevision(value => value + 1); }
  }

  return <section className="space-y-4" data-testid="slack-settings">
    <div>
      <h3 className="text-[13px] font-medium">Slack</h3>
      <p className="mt-1 text-xs leading-relaxed text-muted">Connecte ton compte Slack pour envoyer un message ou son fil dans une nouvelle session Chauffeur.</p>
    </div>
    <div className="space-y-3 rounded-lg border border-line p-3">
      {!status && !error && <p role="status" className="flex items-center gap-2 text-xs"><Spinner /> Vérification de la connexion…</p>}
      {status?.identity ? <>
        <p role="status" className="flex items-center gap-2 text-xs"><Check size={14} className="text-success" /> Connecté en tant que {status.identity.name}</p>
        <p className="text-[11px] text-muted">Workspace Dialog · {status.identity.user_id}</p>
        <Button size="sm" busy={busy} onClick={() => void act("logout")}>Se déconnecter</Button>
      </> : status?.pending ? <>
        <p role="status" className="flex items-center gap-2 text-xs"><Spinner /> Termine la connexion dans ton navigateur…</p>
        <Button size="sm" busy={busy} onClick={() => void act("cancel")}>Annuler la connexion</Button>
      </> : <>
        <Button variant="primary" size="sm" busy={busy} onClick={() => void act("login")} data-testid="slack-login"><MessageSquare size={14} /> Se connecter avec Slack</Button>
        <p className="text-[11px] leading-relaxed text-muted">Utilise ton compte du workspace Dialog. Aucun token ni secret à copier.</p>
      </>}
    </div>
    <p className="text-xs leading-relaxed text-muted">Dans Slack, ouvre le menu ⋯ d’un message, puis « Ouvrir dans Chauffeur ». Le fil s’ouvre directement dans le dernier Chauffeur utilisé parmi tes ordinateurs connectés. Garde l’app ouverte. Tu choisis tes repos avant de lancer la session.</p>
    <p className="text-[11px] leading-relaxed text-muted">Pour importer un fil complet, ajoute l’app Chauffeur au channel. Les pièces jointes restent des liens Slack.</p>
    {error && <p role="alert" className="text-xs leading-relaxed text-danger selectable">{error}</p>}
  </section>;
}
