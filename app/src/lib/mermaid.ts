// Mermaid diagrams an agent writes (```mermaid), drawn as SVG. The library is large: it is loaded
// on the first diagram. Drawings are kept by source, since the chat list unmounts and remounts rows.

type Mermaid = (typeof import("mermaid"))["default"];

let library: Promise<Mermaid> | null = null;
let queue: Promise<unknown> = Promise.resolve();
let counter = 0;
const drawn = new Map<string, Promise<string>>();

function load(): Promise<Mermaid> {
  library ??= import("mermaid").then(({ default: mermaid }) => {
    mermaid.initialize({
      startOnLoad: false,
      // Labels as text, no click handlers or scripts: the agent's diagram is data, not code.
      securityLevel: "strict",
      htmlLabels: false,
      theme: "dark",
      darkMode: true,
      fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif",
      themeVariables: { background: "#17171a", fontSize: "13px" },
    });
    return mermaid;
  });
  return library;
}

/** The SVG of a diagram; rejects with Mermaid's message when the source is not a valid diagram. */
export function drawMermaid(source: string): Promise<string> {
  const known = drawn.get(source);
  if (known) return known;
  // Mermaid keeps global state while it draws: one diagram at a time.
  const result = queue.then(async () => {
    const mermaid = await load();
    const id = `mermaid-${++counter}`;
    try {
      await mermaid.parse(source);
      const { svg } = await mermaid.render(id, source);
      return svg;
    } finally {
      // A failed render leaves its scratch element in the page.
      document.getElementById(`d${id}`)?.remove();
      document.getElementById(id)?.remove();
    }
  });
  queue = result.catch(() => undefined);
  drawn.set(source, result);
  result.catch(() => drawn.delete(source));
  return result;
}
