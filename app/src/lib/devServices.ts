// A workspace's development services (docs/workspace-development-services.md): what detection
// found, what each can be declared as, and the choices the dialog starts from; and its web
// previews, the ports of its block that answer HTTP, which the user checks to declare. French or
// English, as the transfer copy is.
import type { DetectedPreview, DevServiceChoice, DetectedService, DevServices, PreviewChoice } from "@chauffeur/protocol";

export type ServiceChoice = "data" | "cache" | "skip";

export function devServicesCopy(french: boolean) {
  return french
    ? {
        menu: "Services de développement…",
        title: (name: string) => `Services de développement de ${name}`,
        intro:
          "Les conteneurs Docker qui travaillent dans ce workspace. Quand il part sur une autre machine, ils sont d'abord arrêtés ici : les bases d'un serveur PostgreSQL déclaré comme données sont exportées et partent avec lui ; un cache ne part pas.",
        detecting: "Détection des conteneurs…",
        none: "Aucun conteneur ne travaille dans ce workspace.",
        data: "Données",
        dataHint: (databases: string[]) => `Exporter ${databases.join(", ")}`,
        cache: "Cache",
        cacheHint: "Arrêté, sans rien emporter",
        skip: "Ignorer",
        skipHint: "Un transfert est refusé tant qu'il tourne",
        stopped: "arrêté",
        noCompose: "sans fichier compose",
        save: "Enregistrer",
        cancel: "Annuler",
        saved: "Services de développement enregistrés",
        detectAgain: "Détecter à nouveau",
        keepData: (name: string) =>
          `${name} ne peut pas prendre le type choisi maintenant (voir ci-dessus) : corrigez-le puis détectez à nouveau, ou choisissez un autre type.`,
        previews: "Aperçus web",
        previewsIntro:
          "Les ports du workspace qui répondent en HTTP. Ceux cochés s'ouvrent depuis le téléphone, par le canal chiffré, pendant que le workspace tourne sur une autre machine.",
        noPreviews: "Aucun port du workspace ne répond en HTTP : lancez son serveur de développement, puis détectez à nouveau.",
        silent: "rien ne répond",
        previewName: (port: number) => `Nom de l'aperçu du port ${port}`,
        badPreviewName: (port: number) =>
          `Nommez l'aperçu du port ${port} avec des lettres, des chiffres, _ ou -, un nom par aperçu.`,
      }
    : {
        menu: "Development services…",
        title: (name: string) => `Development services of ${name}`,
        intro:
          "The Docker containers that work in this workspace. When it moves to another machine, they are stopped here first: the databases of a PostgreSQL server declared as data are exported and travel with it; a cache does not.",
        detecting: "Detecting containers…",
        none: "No container works in this workspace.",
        data: "Data",
        dataHint: (databases: string[]) => `Export ${databases.join(", ")}`,
        cache: "Cache",
        cacheHint: "Stopped, nothing carried",
        skip: "Leave out",
        skipHint: "A transfer is refused while it runs",
        stopped: "stopped",
        noCompose: "no compose file",
        save: "Save",
        cancel: "Cancel",
        saved: "Development services saved",
        detectAgain: "Detect again",
        keepData: (name: string) =>
          `${name} cannot take the chosen kind now (see above): fix it and detect again, or choose another kind.`,
        previews: "Web previews",
        previewsIntro:
          "The ports of the workspace that answer HTTP. Those checked open from the phone, over the encrypted channel, while the workspace runs on another machine.",
        noPreviews: "No port of the workspace answers HTTP: start its dev server, then detect again.",
        silent: "nothing answers",
        previewName: (port: number) => `Name of the preview on port ${port}`,
        badPreviewName: (port: number) =>
          `Name the preview on port ${port} with letters, digits, _ or -, one name per preview.`,
      };
}

/** What a detected service can be declared as: data only for a running PostgreSQL server with
 * databases and a compose file; nothing when Docker could not stop and start it exactly. */
export function allowedChoices(service: DetectedService): ServiceChoice[] {
  const pinnable = !(service.problem && service.compose);
  const data =
    pinnable &&
    !service.problem &&
    service.running &&
    (service.postgres?.databases.length ?? 0) > 0;
  return [...(data ? (["data"] as const) : []), ...(pinnable ? (["cache"] as const) : []), "skip"];
}

/** Where the dialog starts: what is declared, else what detection suggests; every container that
 * works in the workspace is declared by default, since a transfer refuses one left running. */
export function initialChoices(detected: DetectedService[], declared: DevServices | null): Record<string, ServiceChoice> {
  const choices: Record<string, ServiceChoice> = {};
  for (const service of detected) {
    const allowed = allowedChoices(service);
    const known = declared?.services.find((s) => s.name === service.name);
    // Declared, it keeps its kind even when it cannot take it now (stopped, its container gone):
    // saving then waits for an explicit choice rather than dropping it unseen.
    if (known) {
      choices[service.name] = known.data ? "data" : "cache";
      continue;
    }
    const wanted: ServiceChoice = service.data ? "data" : "cache";
    choices[service.name] = allowed.includes(wanted) ? wanted : allowed.includes("cache") ? "cache" : "skip";
  }
  return choices;
}

/** The choices as saved: the services left out are not sent. */
export function savedChoices(choices: Record<string, ServiceChoice>): DevServiceChoice[] {
  return Object.entries(choices)
    .filter(([, choice]) => choice !== "skip")
    .map(([name, choice]) => ({ name, data: choice === "data" }));
}

/** The services whose choice they cannot take now (a declared server stopped, a declared container
 * gone): saving waits for another choice. */
export function blockedChoices(detected: DetectedService[], choices: Record<string, ServiceChoice>): string[] {
  return detected
    .filter((service) => !allowedChoices(service).includes(choices[service.name] ?? "skip"))
    .map((service) => service.name);
}

/** A detected port as the dialog shows it: declared or not, and under which name. */
export type PreviewDraft = { name: string; declare: boolean };

/** Where the previews start: those declared stay checked under their name; a port that answers
 * now is offered under the name detection suggests, unchecked until the user confirms it. */
export function initialPreviews(detected: DetectedPreview[]): Record<number, PreviewDraft> {
  return Object.fromEntries(detected.map((p) => [p.port, { name: p.name, declare: p.declared }]));
}

/** The previews as saved: every checked port under its name. */
export function savedPreviews(drafts: Record<number, PreviewDraft>): PreviewChoice[] {
  return Object.entries(drafts)
    .filter(([, draft]) => draft.declare)
    .map(([port, draft]) => ({ name: draft.name.trim(), port: Number(port) }));
}

/** The checked ports whose name the daemon would refuse: empty, other than letters, digits, `_`
 * and `-`, or taken by another checked port. */
export function badPreviewNames(drafts: Record<number, PreviewDraft>): number[] {
  const chosen = savedPreviews(drafts);
  return chosen
    .filter((p) => !/^[A-Za-z0-9_-]{1,80}$/.test(p.name) || chosen.filter((q) => q.name === p.name).length > 1)
    .map((p) => p.port);
}
