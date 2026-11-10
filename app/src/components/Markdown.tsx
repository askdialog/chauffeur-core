import { createContext, memo, useContext, useState } from "react";
import type { Element } from "hast";
import ReactMarkdown, { type Components, defaultUrlTransform } from "react-markdown";
import { codeBlockText } from "@/lib/codeBlock";
import { highlight } from "@/lib/highlight";
import { markdownBlocks, remarkPlugins } from "@/lib/markdown";
import { openExternal } from "@/lib/native";
import { CopyCodeButton } from "./CopyCodeButton";
import { CopyTableButton } from "./CopyTableButton";
import { tableTsv } from "@/lib/tableTsv";
import { type FileLinker, REVEAL_LABEL, useFileLinker, useFileMenu } from "./FileLinks";
import { Lightbox, type LightboxImage } from "./Lightbox";
import { MermaidDiagram } from "./MermaidDiagram";

/** The message is still arriving: a diagram is drawn once its source is complete. */
const StreamingContext = createContext(false);
/** Inside a link: an image follows the link rather than opening full screen. */
const LinkedContext = createContext(false);

/** The source of a ```mermaid block, or null for any other block. */
function mermaidSource(pre: Element | undefined): string | null {
  const code = pre?.children[0];
  if (!code || code.type !== "element" || code.tagName !== "code") return null;
  const classes = code.properties?.className;
  if (!Array.isArray(classes) || !classes.includes("language-mermaid")) return null;
  return code.children.map((child) => (child.type === "text" ? child.value : "")).join("");
}

function Pre({ node, children }: { node?: Element; children?: React.ReactNode }) {
  const streaming = useContext(StreamingContext);
  const source = mermaidSource(node);
  if (source != null) return <MermaidDiagram source={source} streaming={streaming} />;
  return (
    <div className="md-code group/code relative">
      <pre>{children}</pre>
      <div className="absolute top-1.5 right-1.5 opacity-0 transition-opacity group-hover/code:opacity-100 focus-within:opacity-100">
        <CopyCodeButton text={codeBlockText(node)} />
      </div>
    </div>
  );
}

const components: Components = {
  table({ node, children, ...props }) {
    return (
      <div className="group/table relative w-fit max-w-full pr-7">
        {node && <CopyTableButton text={tableTsv(node)} />}
        <table {...props}>{children}</table>
      </div>
    );
  },
  pre({ node, children }) {
    return <Pre node={node}>{children}</Pre>;
  },
  code({ className, children }) {
    const match = /language-([\w+-]+)/.exec(className ?? "");
    const text = String(children ?? "");
    const block = match || text.includes("\n");
    if (!block) return <InlineCode text={text}>{children}</InlineCode>;
    return (
      <code
        className={className}
        dangerouslySetInnerHTML={{ __html: highlight(text.replace(/\n$/, ""), match?.[1]) }}
      />
    );
  },
  a({ href, children }) {
    return <Link href={href}>{children}</Link>;
  },
  img({ src, alt }) {
    return <Image src={typeof src === "string" ? src : undefined} alt={alt} />;
  },
};

/** Where an image the markdown names is loaded from: a chat serves local files through the daemon. */
export const ImageSourceContext = createContext<((src: string) => string | null) | null>(null);

function Image({ src, alt }: { src: string | undefined; alt: string | undefined }) {
  const resolve = useContext(ImageSourceContext);
  const linked = useContext(LinkedContext);
  const linker = useFileLinker();
  const { onContextMenu, menu } = useFileMenu(linker);
  const [open, setOpen] = useState<{ images: LightboxImage[]; start: number } | null>(null);
  const local = src && !/^(https?:|data:|blob:)/.test(src) ? safeDecode(src) : null;
  const url = local && resolve ? resolve(local) : src;
  if (!url) return <span className="text-faint">{alt || src}</span>;
  return (
    <>
      <img
        src={url}
        alt={alt ?? ""}
        title={alt || src}
        className={linked ? "md-image" : "md-image cursor-zoom-in"}
        loading="lazy"
        data-zoomable={linked ? undefined : ""}
        onClick={
          linked
            ? undefined
            : (event) => {
                // The message's other images are a step away, in their order on the page.
                const all = Array.from(event.currentTarget.closest(".md")?.querySelectorAll<HTMLImageElement>("img[data-zoomable]") ?? []);
                setOpen({ images: all.map((img) => ({ src: img.src, name: img.title })), start: all.indexOf(event.currentTarget) });
              }
        }
        onContextMenu={onContextMenu(local ? (linker?.resolve(local) ?? null) : null)}
      />
      {open && <Lightbox images={open.images} start={open.start} onClose={() => setOpen(null)} />}
      {menu}
    </>
  );
}

/** Inline code naming a file of the workspace opens it. */
function InlineCode({ text, children }: { text: string; children: React.ReactNode }) {
  const linker = useFileLinker();
  const { onContextMenu, menu } = useFileMenu(linker);
  const ref = linker?.resolve(text);
  const folder = !ref && linker ? linker.resolveFolder(text) : null;
  if (!linker || !(ref || folder)) return <code>{children}</code>;
  const activate = () => (ref ? linker.open(ref) : folder && linker.revealPath(folder));
  return (
    <>
      <code
        role="link"
        tabIndex={0}
        className="file-link"
        title={ref ? `Open ${ref.path}${ref.line ? `:${ref.line}` : ""}` : REVEAL_LABEL}
        onClick={activate}
        onContextMenu={onContextMenu(ref ?? null)}
        onKeyDown={(event) => {
          if (event.key === "Enter") activate();
        }}
      >
        {children}
      </code>
      {menu}
    </>
  );
}

function Link({ href, children }: { href: string | undefined; children: React.ReactNode }) {
  const linker = useFileLinker();
  const { onContextMenu, menu } = useFileMenu(linker);
  const local = href && !/^https?:\/\//.test(href) ? safeDecode(href) : null;
  const ref = local ? linker?.resolve(local) : null;
  const folder = local && !ref ? linker?.resolveFolder(local) : null;
  return (
    <>
      <a
        href={href}
        title={ref ? `Open ${ref.path}${ref.line ? `:${ref.line}` : ""}` : folder ? REVEAL_LABEL : undefined}
        onClick={(event) => {
          event.preventDefault();
          if (ref && linker) linker.open(ref);
          else if (folder && linker) linker.revealPath(folder);
          else if (href && /^https?:\/\//.test(href)) void openExternal(href);
          else if (href && linker) void openUnlisted(linker, safeDecode(href));
        }}
        onContextMenu={onContextMenu(ref ?? null)}
      >
        <LinkedContext.Provider value={true}>{children}</LinkedContext.Provider>
      </a>
      {menu}
    </>
  );
}

/**
 * A file missing from the chat's list of files may be newer than it: list them again before giving
 * up. A folder of the workspace is not in the list: Finder shows it, or says it is not there.
 */
async function openUnlisted(linker: FileLinker, path: string) {
  const ref = await linker.resolveFresh(path);
  if (ref) linker.open(ref);
  else linker.revealPath(path);
}

/** Markdown drops `file:` links as unsafe; the chat's file links must reach `Link`. */
function urlTransform(url: string): string {
  return /^file:\/\//i.test(url) ? url : defaultUrlTransform(url);
}

const Block = memo(function Block({ text }: { text: string }) {
  return (
    <ReactMarkdown remarkPlugins={remarkPlugins} components={components} urlTransform={urlTransform}>
      {text}
    </ReactMarkdown>
  );
});

/**
 * Agent markdown; `streaming` shows a caret after the last block while text arrives. A streamed
 * message is drawn in pieces (`markdownBlocks`): each delta parses and highlights only the last one.
 */
export const Markdown = memo(function Markdown({ text, streaming = false }: { text: string; streaming?: boolean }) {
  const blocks = streaming ? markdownBlocks(text) : [text];
  return (
    <div className={streaming ? "md md-streaming selectable" : "md selectable"}>
      <StreamingContext.Provider value={streaming}>
        {blocks.map((block, index) => (
          <Block key={index} text={block} />
        ))}
      </StreamingContext.Provider>
    </div>
  );
});

function safeDecode(href: string): string {
  try {
    return decodeURI(href);
  } catch {
    return href;
  }
}
