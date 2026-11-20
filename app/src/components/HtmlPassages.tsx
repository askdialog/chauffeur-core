import { useLayoutEffect, useRef } from "react";
import { QUOTE_MAX, quoteOf } from "@/lib/shareComments";

export type PassageSelection = { quote: string; top: number; left: number };
export type HtmlPreview = { url: string; title: string; channel: string };
type Anchor = { id: string; quote: string; resolved: boolean };
type AnnotationState = { anchors: Anchor[]; active: string | null; focus: number; clear: number };

// Serialized into the sandbox before document-supplied scripts. Everything it needs is local.
function annotateDocument(channel: string, maxQuote: number) {
  document.currentScript?.remove();
  const { port1, port2 } = new MessageChannel();
  const send = port1.postMessage.bind(port1);
  window.parent.postMessage({ type: "html-passages", channel }, "*", [port2]);
  let state: AnnotationState = { anchors: [], active: null, focus: 0, clear: 0 };
  let ranges = new Map<string, Range>();
  const all = "chauffeur-passages";
  const current = "chauffeur-active-passage";

  function rebuild() {
    if (!document.body) return;
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT, {
      acceptNode: (node) => node.parentElement?.closest("script, style, noscript, textarea, [hidden]")
        ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT,
    });
    const nodes: { node: Text; start: number; end: number }[] = [];
    let text = "";
    for (let next = state.anchors.length ? walker.nextNode() : null; next; next = walker.nextNode()) {
      const node = next as Text;
      const compact = node.data.replace(/\s/g, "");
      if (!compact) continue;
      nodes.push({ node, start: text.length, end: text.length + compact.length });
      text += compact;
    }
    function point(index: number) {
      const entry = nodes.find((item) => item.end > index);
      if (!entry) return null;
      let remaining = index - entry.start;
      for (let offset = 0; offset < entry.node.length; offset += 1) {
        if (!/\s/.test(entry.node.data.charAt(offset)) && remaining-- === 0) return { node: entry.node, offset };
      }
      return null;
    }
    ranges = new Map();
    const missing: string[] = [];
    const open: Range[] = [];
    for (const anchor of state.anchors) {
      const quote = anchor.quote.replace(/\s/g, "");
      const index = quote ? text.indexOf(quote) : -1;
      const start = index < 0 ? null : point(index);
      const end = index < 0 ? null : point(index + quote.length - 1);
      if (!start || !end) { missing.push(anchor.id); continue; }
      const range = document.createRange();
      range.setStart(start.node, start.offset);
      range.setEnd(end.node, end.offset + 1);
      ranges.set(anchor.id, range);
      if (!anchor.resolved) open.push(range);
    }
    if (typeof Highlight !== "undefined" && CSS.highlights) {
      const active = state.active ? ranges.get(state.active) : undefined;
      CSS.highlights.set(all, new Highlight(...open));
      if (active) CSS.highlights.set(current, new Highlight(active));
      else CSS.highlights.delete(current);
    }
    send({ type: "missing", ids: missing });
  }

  port1.addEventListener("message", (event: MessageEvent<AnnotationState>) => {
    const previous = state;
    state = event.data;
    if (state.clear !== previous.clear) window.getSelection()?.removeAllRanges();
    rebuild();
    if (state.focus !== previous.focus && state.active) {
      ranges.get(state.active)?.startContainer.parentElement?.scrollIntoView({ block: "center", behavior: "smooth" });
    }
  });
  port1.start();
  document.addEventListener("DOMContentLoaded", () => {
    const style = document.createElement("style");
    style.textContent = "::highlight(chauffeur-passages){background-color:#f0c75a66}::highlight(chauffeur-active-passage){background-color:#f0b838aa}";
    document.head.append(style);
    rebuild();
    let pending = false;
    new MutationObserver(() => {
      if (pending || !state.anchors.length) return;
      pending = true;
      requestAnimationFrame(() => { pending = false; rebuild(); });
    }).observe(document.body, { subtree: true, childList: true, characterData: true });
  });

  let pendingSelection = false;
  const reportSelection = () => {
    if (pendingSelection) return;
    pendingSelection = true;
    requestAnimationFrame(() => {
      pendingSelection = false;
      const selection = window.getSelection();
      const quote = selection?.toString().trim();
      if (!selection || !quote || quote.length > maxQuote || !selection.rangeCount) {
        send({ type: "selection" });
        return;
      }
      const box = selection.getRangeAt(0).getBoundingClientRect();
      if (box.width && box.height && box.bottom > 0 && box.top < window.innerHeight) {
        send({ type: "selection", quote, top: box.bottom, left: box.right });
      } else send({ type: "selection" });
    });
  };
  document.addEventListener("selectionchange", reportSelection);
  window.addEventListener("scroll", reportSelection, true);
  window.addEventListener("resize", reportSelection);
  document.addEventListener("click", (event) => {
    if (!event.isTrusted || !window.getSelection()?.isCollapsed) return;
    const caret = document.caretRangeFromPoint?.(event.clientX, event.clientY);
    if (!caret) return;
    for (const { id, resolved } of state.anchors) {
      if ((!resolved || id === state.active) && ranges.get(id)?.isPointInRange(caret.startContainer, caret.startOffset)) {
        send({ type: "thread", id });
        return;
      }
    }
  });
}

export function htmlPassageScript(channel: string): string {
  return `<script>(${annotateDocument.toString()})(${JSON.stringify(channel)},${QUOTE_MAX})</script>`;
}

export function HtmlPassages({ preview, annotations, onSelection, onThread, onMissing }: {
  preview: HtmlPreview;
  annotations: AnnotationState;
  onSelection: (selection: PassageSelection | null) => void;
  onThread: (id: string) => void;
  onMissing: (ids: string[]) => void;
}) {
  const frame = useRef<HTMLIFrameElement>(null);
  const port = useRef<MessagePort | null>(null);
  const latest = useRef({ annotations, onSelection, onThread, onMissing });
  latest.current = { annotations, onSelection, onThread, onMissing };
  useLayoutEffect(() => {
    // Only the injected script for this preview knows the handshake value and holds the port.
    const connect = (event: MessageEvent<unknown>) => {
      if (event.source !== frame.current?.contentWindow || event.origin !== "null" || port.current) return;
      const data = event.data;
      if (!data || typeof data !== "object" || !("type" in data) || data.type !== "html-passages"
        || !("channel" in data) || data.channel !== preview.channel || event.ports.length !== 1) return;
      const connection = event.ports[0];
      port.current = connection;
      connection.onmessage = ({ data }: MessageEvent<unknown>) => {
        if (!data || typeof data !== "object" || !("type" in data)) return;
        const { annotations, onSelection, onThread, onMissing } = latest.current;
        if (data.type === "selection") {
          const box = frame.current?.getBoundingClientRect();
          if (!box || !("quote" in data) || typeof data.quote !== "string" || data.quote.length > QUOTE_MAX
            || !("top" in data) || typeof data.top !== "number" || !Number.isFinite(data.top)
            || !("left" in data) || typeof data.left !== "number" || !Number.isFinite(data.left)) {
            onSelection(null);
            return;
          }
          const quote = quoteOf(data.quote);
          onSelection(quote ? {
            quote,
            top: Math.max(box.top, Math.min(box.top + data.top, box.bottom - 34)),
            left: Math.max(box.left, Math.min(box.left + data.left, box.right)),
          } : null);
        } else if (data.type === "thread" && "id" in data && typeof data.id === "string") {
          const id = data.id;
          if (annotations.anchors.some((anchor) => anchor.id === id)) onThread(id);
        } else if (data.type === "missing" && "ids" in data && Array.isArray(data.ids)) {
          const ids: unknown[] = data.ids;
          onMissing(annotations.anchors.filter((anchor) => ids.includes(anchor.id)).map((anchor) => anchor.id));
        }
      };
      connection.postMessage(latest.current.annotations);
    };
    window.addEventListener("message", connect);
    return () => {
      window.removeEventListener("message", connect);
      port.current?.close();
      port.current = null;
    };
  }, [preview.channel]);
  useLayoutEffect(() => port.current?.postMessage(annotations), [annotations]);
  return <iframe key={preview.channel} ref={frame} src={preview.url} title={preview.title}
    sandbox="allow-scripts" referrerPolicy="no-referrer" className="h-full w-full border-0 bg-white" />;
}
