import { Code2, Maximize2, Workflow } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { highlight } from "@/lib/highlight";
import { drawMermaid } from "@/lib/mermaid";
import { CopyCodeButton } from "./CopyCodeButton";
import { Lightbox } from "./Lightbox";

/**
 * A ```mermaid block drawn as a diagram, its source a click away; full screen (zoomable) on demand.
 * While the message streams, or when the source is not a valid diagram, the code shows.
 */
export function MermaidDiagram({ source, streaming }: { source: string; streaming: boolean }) {
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showCode, setShowCode] = useState(false);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (streaming) return;
    let alive = true;
    setError(null);
    drawMermaid(source).then(
      (drawnSvg) => alive && setSvg(drawnSvg),
      (err: unknown) => alive && setError(err instanceof Error ? err.message.split("\n")[0] : String(err)),
    );
    return () => {
      alive = false;
    };
  }, [source, streaming]);

  const url = useMemo(() => (svg ? URL.createObjectURL(new Blob([fullScreenSvg(svg)], { type: "image/svg+xml" })) : null), [svg]);
  useEffect(() => () => {
    if (url) URL.revokeObjectURL(url);
  }, [url]);

  // Find in a chat leaves a diagram out, its code too (lib/markdown.ts).
  const code = (
    <pre data-find-skip>
      <code className="language-mermaid" dangerouslySetInnerHTML={{ __html: highlight(source.replace(/\n$/, ""), "mermaid") }} />
    </pre>
  );
  if (streaming || (!svg && !error)) return code;
  if (error) {
    return (
      <div data-find-skip>
        {code}
        <div className="-mt-1.5 mb-2 text-[11.5px] text-faint" title={error}>
          Not drawn: {error}
        </div>
      </div>
    );
  }
  return (
    <div className="md-mermaid group relative" data-mermaid data-find-skip>
      <div className="absolute top-1.5 right-1.5 z-10 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
        <button type="button" className="md-block-button" onClick={() => setShowCode((v) => !v)} title={showCode ? "Show the diagram" : "Show its code"}>
          {showCode ? <Workflow size={12} /> : <Code2 size={12} />}
          {showCode ? "Diagram" : "Code"}
        </button>
        {showCode && <CopyCodeButton text={source.replace(/\n$/, "")} />}
        {!showCode && (
          <button type="button" className="md-block-button" onClick={() => setOpen(true)} title="Full screen, to zoom in">
            <Maximize2 size={12} />
          </button>
        )}
      </div>
      {showCode ? (
        code
      ) : (
        <button type="button" className="block w-full cursor-zoom-in overflow-x-auto p-3" onClick={() => setOpen(true)} title="Full screen, to zoom in">
          {/* Mermaid's own sanitised SVG (strict security level). */}
          <div className="md-mermaid-svg" dangerouslySetInnerHTML={{ __html: svg! }} />
        </button>
      )}
      {open && <Lightbox images={[{ src: url, name: "Diagram" }]} onClose={() => setOpen(false)} />}
    </div>
  );
}

/**
 * The diagram as an image file for the full-screen view: Mermaid sizes its SVG to the chat's width
 * (`width="100%"`, a max-width), which an image has not; it gets its own size, large enough to fill
 * a screen before any zoom.
 */
function fullScreenSvg(svg: string): string {
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  const root = doc.documentElement;
  const box = root.getAttribute("viewBox")?.split(/[\s,]+/).map(Number);
  if (root.nodeName !== "svg" || !box || box.length !== 4 || box.some(Number.isNaN)) return svg;
  const [, , width, height] = box;
  const scale = Math.max(1, 1600 / Math.max(width, height));
  root.setAttribute("width", String(Math.round(width * scale)));
  root.setAttribute("height", String(Math.round(height * scale)));
  root.style.removeProperty("max-width");
  // Opaque: the chat must not show through it.
  root.style.setProperty("background-color", "#17171a");
  return new XMLSerializer().serializeToString(doc);
}
