/** Enabled, visible controls in DOM order, including portalled pickers and global recovery actions. */
export function dialogControls(panel: HTMLElement): HTMLElement[] {
  const roots = [panel, ...document.querySelectorAll<HTMLElement>("[data-popover], [data-global-feedback]")];
  return roots.flatMap((root) => [...root.querySelectorAll<HTMLElement>(
    'button, summary, input:not([type="hidden"]), select, textarea, a[href], [tabindex], [contenteditable="true"]',
  )]).filter((element) => {
    if ((element.tabIndex < 0 && !element.matches("summary:not([tabindex])")) || element.matches(":disabled") || element.closest("[inert], [hidden]")) return false;
    for (let node: HTMLElement | null = element; node; node = node.parentElement) {
      if (node.matches("details:not([open])") && !node.querySelector(":scope > summary")?.contains(element)) return false;
      const style = window.getComputedStyle(node);
      if (style.display === "none" || style.visibility === "hidden") return false;
    }
    return true;
  });
}
