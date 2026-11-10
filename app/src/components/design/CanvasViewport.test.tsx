import { expect, spyOn, test } from "bun:test";
import { act, type ComponentProps } from "react";
import { Window } from "happy-dom";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { layoutCanvas } from "@/lib/canvas";
import { CanvasViewport } from "./CanvasViewport";

const noop = () => {};
const props: ComponentProps<typeof CanvasViewport> = {
  layout: layoutCanvas([{
    title: "Large canvas",
    unsorted: false,
    screens: Array.from({ length: 100 }, (_, i) => ({
      file: `${i}.html`, width: 390, height: 844, url: `https://screens.test/${i}.html`, modified_ms: 1,
    })),
  }]),
  zoom: { scale: 1, x: 0, y: 0 },
  area: { width: 800, height: 600 },
  setZoom: noop, onArea: noop, selection: ["50.html"], onSelect: noop, flash: {},
  onFullScreen: noop, onFit: noop, onActualSize: noop, published: {}, commentMode: false,
  pins: {}, draft: null, openThread: null, onFrame: noop, onCommentAt: noop,
  onOpenThread: noop, onEscape: noop, onToggleCommentMode: noop, popover: null,
};

test("a loaded canvas renders nearby frames and retains the full section count", () => {
  const html = renderToStaticMarkup(<CanvasViewport {...props} />);
  expect(html.match(/<iframe /g)).toHaveLength(3);
  expect(html).toContain('src="https://screens.test/0.html?v=1&amp;capture=1"');
  expect(html).not.toContain('data-file="50.html"');
  expect(html).toContain('>100</span>');
  expect(html.match(/sandbox="allow-scripts allow-forms"/g)).toHaveLength(3);
});

test("panning to a selected screen restores its frame and selection outline", () => {
  const html = renderToStaticMarkup(<CanvasViewport {...props} zoom={{ scale: 1, x: -50 * 430, y: 0 }} />);
  expect(html.match(/<iframe /g)).toHaveLength(4);
  expect(html).toContain('data-file="50.html"');
  expect(html).toContain('data-testid="canvas-selected"');
  expect(html).not.toContain('data-file="0.html"');
});

test("the first render waits for viewport measurement without losing the section header", () => {
  const html = renderToStaticMarkup(<CanvasViewport {...props} area={null} />);
  expect(html).not.toContain('<iframe');
  expect(html).toContain('Large canvas');
});

test("an offscreen comment keeps its composer mounted while its frame is released", () => {
  const html = renderToStaticMarkup(<CanvasViewport
    {...props}
    draft={{ file: "50.html", x: 20, y: 30, whole: false, element: null }}
    popover={<textarea defaultValue="Comment in progress" />}
  />);
  expect(html).toContain('Comment in progress');
  expect(html.match(/<iframe /g)).toHaveLength(3);
});

test("repeated files do not accumulate ghost screens when snapshots appear and disappear", async () => {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, ResizeObserver: browser.ResizeObserver, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const errors = spyOn(console, "error").mockImplementation(() => {});
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  let trackedFrame: HTMLIFrameElement | null = null;
  const onFrame = (_file: string, frame: HTMLIFrameElement | null) => { trackedFrame = frame; };
  const screen = { file: "oden.html", url: "about:blank", width: 390, height: 844 };
  const layout = layoutCanvas([{ title: "Repeated", unsorted: false, screens: [screen, { ...screen, height: 1120 }] }]);
  try {
    for (let i = 0; i < 12; i++) {
      const snapshots = i % 3 === 0 ? {} : Object.fromEntries(layout.sections[0].screens.map((placed) => [placed.id, {
        src: "about:blank", url: "data:image/png;base64,", rect: { x: 0, y: 0, width: placed.width, height: placed.height }, density: 1, whole: true,
      }]));
      await act(() => root.render(<CanvasViewport {...props} layout={layout} onFrame={onFrame} selection={[]} zoom={{ scale: i % 2 ? 0.44 : 0.3, x: 0, y: 0 }} snapshots={snapshots} />));
      expect(container.querySelectorAll("iframe")).toHaveLength(2);
      expect(container.querySelectorAll('[data-testid="canvas-snapshots"] img')).toHaveLength(i % 3 === 0 ? 0 : 2);
    }
    await act(() => root.render(<CanvasViewport {...props} layout={layout} onFrame={onFrame} zoom={{ scale: 1, x: 0, y: 0 }} area={{ width: 100, height: 600 }} />));
    expect(container.querySelectorAll("iframe")).toHaveLength(1);
    expect(trackedFrame === container.querySelector("iframe")).toBe(true);
    expect(errors.mock.calls.filter((args) => String(args[0]).includes("same key"))).toEqual([]);
  } finally {
    await act(() => root.unmount());
    errors.mockRestore();
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});

test("the wheel over a comment scrolls its text, and pans the canvas only outside it", async () => {
  const browser = new Window();
  const globals = {
    window: browser, document: browser.document, ResizeObserver: browser.ResizeObserver, Element: browser.Element,
    HTMLTextAreaElement: browser.HTMLTextAreaElement, getComputedStyle: browser.getComputedStyle.bind(browser),
    requestAnimationFrame: (run: FrameRequestCallback) => { run(0); return 1; }, cancelAnimationFrame: () => {},
    IS_REACT_ACT_ENVIRONMENT: true,
  };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const pans: unknown[] = [];
  try {
    await act(() => root.render(<CanvasViewport
      {...props}
      setZoom={(update) => pans.push(update)}
      draft={{ file: "0.html", x: 20, y: 30, whole: false, element: null }}
      popover={<textarea defaultValue="A long comment" />}
    />));
    const box = container.querySelector("textarea");
    if (!box) throw new Error("no textarea in the popover");
    Object.defineProperties(box, { scrollHeight: { value: 120 }, clientHeight: { value: 40 } });
    const wheel = (target: Element, deltaY: number) => {
      const event = new browser.WheelEvent("wheel", { deltaY, bubbles: true, cancelable: true });
      target.dispatchEvent(event as unknown as Event);
      return event.defaultPrevented;
    };

    expect(wheel(box, 30)).toBe(false);
    box.scrollTop = 80;
    expect(wheel(box, 30)).toBe(true);
    expect(wheel(box, -30)).toBe(false);
    expect(pans).toHaveLength(0);

    const viewport = container.querySelector('[data-testid="canvas-viewport"]');
    if (!viewport) throw new Error("no viewport");
    expect(wheel(viewport, 30)).toBe(true);
    expect(pans).toHaveLength(1);
  } finally {
    await act(() => root.unmount());
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
