// A web link that no component opened: a middle click, or a modified click, on a link of the chat,
// or a link a component draws without a handler. The web view's default is to load the site in
// place of the app (WebKitGTK does), which leaves no way back but a restart: it opens in the
// user's browser instead.

type ClickLike = Pick<MouseEvent, "defaultPrevented" | "button" | "preventDefault"> & { target: unknown };

/** What a click lands on, as far as this needs it: an element, or a text node (which has no `closest`). */
type Landing = { closest?: (selector: string) => { getAttribute(name: string): string | null } | null };

/** The web address a click on the window would follow by default, or null. */
export function strayLink(event: Pick<ClickLike, "defaultPrevented" | "button" | "target">): string | null {
  // A component that opened the link (Markdown's) said so; a right click opens a menu.
  if (event.defaultPrevented || event.button > 1) return null;
  // An element, or a text node; typed loosely so that it is read the same without a DOM (tests).
  const href = (event.target as Landing | null)?.closest?.("a[href]")?.getAttribute("href") ?? null;
  return href && /^https?:\/\//i.test(href) ? href : null;
}

/** For `click` and `auxclick` on the window: opens a stray web link with `open` rather than in place. */
export function openStrayLink(event: ClickLike, open: (url: string) => void): void {
  const url = strayLink(event);
  if (!url) return;
  event.preventDefault();
  open(url);
}
