// Signing a GCP worker's agents in from this Mac (docs/cloud-workers.md): the worker's own CLI
// runs the login and its page opens here; Claude Code's page shows a code to paste back, Codex's
// asks for the code shown here. What the app keeps of each login it follows, and what it says of
// it, in French or English as the transfer copy is.
import type { AgentKind, AgentLoginResult, AgentUsage, RegistryEvent, TransferInfo } from "@chauffeur/protocol";

export type WorkerLogin =
  | { step: "starting" }
  | { step: "browser"; url: string; userCode: string | null; asksCode: boolean; sending: boolean }
  | { step: "done" }
  | { step: "failed"; error: string };

export function loginKey(worker: string, agent: AgentKind): string {
  return `${worker}/${agent}`;
}

/** What the page the worker's CLI gave asks of the user. */
export function startedLogin(started: AgentLoginResult): WorkerLogin {
  return {
    step: "browser",
    url: started.url,
    userCode: started.user_code ?? null,
    asksCode: started.asks_code ?? false,
    sending: false,
  };
}

/** The logins after a registry event: a worker login under way here ended. Null when the event
 * changes none of them (another machine's login, one not followed, one already over). */
export function afterLoginEnded(logins: Record<string, WorkerLogin>, event: RegistryEvent): Record<string, WorkerLogin> | null {
  if (event.type !== "login_ended" || !event.worker) return null;
  const key = loginKey(event.worker, event.agent);
  const login = logins[key];
  if (!login || login.step === "done" || login.step === "failed") return null;
  return {
    ...logins,
    [key]: event.success ? { step: "done" } : { step: "failed", error: event.error ?? "the login did not complete" },
  };
}

/** Whether a transfer refused for want of a login on its worker goes again: one of the agents it
 * waits for got signed in there from here. */
export function loginResumes(transfer: TransferInfo, logins: Record<string, WorkerLogin>): boolean {
  const needs = transfer.phase === "failed" ? transfer.needs_login : undefined;
  return !!needs && needs.agents.some((agent) => logins[loginKey(needs.worker, agent)]?.step === "done");
}

/** A worker's agents that a sign-in would help: those it reads as signed out. */
export function signedOutAgents(agents: AgentUsage[]): AgentKind[] {
  return agents.filter((u) => u.account === "signed_out").map((u) => u.agent);
}

export function workerLoginCopy(french: boolean) {
  return french
    ? {
        signIn: (agent: string, worker: string) => `Connecter ${agent} sur ${worker}`,
        signInShort: "Connecter",
        opened: "La page de connexion est ouverte dans votre navigateur.",
        enterCode: (code: string) =>
          `Saisissez-y le code ${code}. Si OpenAI refuse la connexion par code, activez-la dans ChatGPT → Settings → Security, puis réessayez.`,
        pasteCode: "Collez ici le code que la page affiche :",
        codePlaceholder: "code#état",
        send: "Valider",
        reopen: "Rouvrir la page",
        copy: "Copier le code",
        cancel: "Annuler",
        waiting: "En attente de la connexion…",
        done: (agent: string, worker: string) => `${agent} est connecté sur ${worker}.`,
        failed: (agent: string, worker: string, error: string) => `${agent} n'est pas connecté sur ${worker} : ${error}`,
        retry: "Réessayer",
        resumes: "Le transfert reprend tout seul dès qu'un agent est connecté sur le worker.",
        tokenNote: "La connexion se fait sur le worker : ses identifiants y restent, jamais sur ce Mac.",
        signedIn: "connecté",
        signedOut: "non connecté",
        unknown: "état inconnu",
        agents: "Agents",
        couldNotStart: (agent: string, worker: string, error: string) => `Impossible de connecter ${agent} sur ${worker} : ${error}`,
        couldNotSend: (error: string) => `Le code n'est pas passé : ${error}`,
      }
    : {
        signIn: (agent: string, worker: string) => `Sign in to ${agent} on ${worker}`,
        signInShort: "Sign in",
        opened: "The login page is open in your browser.",
        enterCode: (code: string) =>
          `Enter the code ${code} there. If OpenAI refuses device code sign-in, turn it on in ChatGPT → Settings → Security, then try again.`,
        pasteCode: "Paste the code the page shows:",
        codePlaceholder: "code#state",
        send: "Submit",
        reopen: "Open the page again",
        copy: "Copy the code",
        cancel: "Cancel",
        waiting: "Waiting for the login…",
        done: (agent: string, worker: string) => `${agent} is signed in on ${worker}.`,
        failed: (agent: string, worker: string, error: string) => `${agent} is not signed in on ${worker}: ${error}`,
        retry: "Try again",
        resumes: "The transfer resumes by itself once one agent is signed in on the worker.",
        tokenNote: "The login happens on the worker: its credentials stay there, never on this Mac.",
        signedIn: "signed in",
        signedOut: "signed out",
        unknown: "unknown",
        agents: "Agents",
        couldNotStart: (agent: string, worker: string, error: string) => `Could not sign ${agent} in on ${worker}: ${error}`,
        couldNotSend: (error: string) => `The code did not go through: ${error}`,
      };
}

export type WorkerLoginCopy = ReturnType<typeof workerLoginCopy>;
