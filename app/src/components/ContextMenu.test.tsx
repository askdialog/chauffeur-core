import { expect } from "bun:test";
import { act, useState } from "react";
import { domTest } from "@/test/dom";
import { ContextMenu } from "./ContextMenu";

domTest("a workspace actions button opens its section menu by keyboard and restores focus", import.meta.path, async ({ render, click }) => {
  function Page() {
    const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
    const [destination, setDestination] = useState("Dialog");
    return (
      <>
        <button id="actions" onClick={(event) => setAnchor(event.currentTarget)}>Actions</button>
        <output>{destination}</output>
        {anchor && (
          <ContextMenu
            x={0}
            y={0}
            anchor={anchor}
            items={[
              { label: "Move to section", submenu: [
                { label: "Dialog", disabled: true, current: true },
                { label: "Chauffeur", onSelect: () => setDestination("Chauffeur") },
              ] },
              { label: "Archive", onSelect: () => {} },
            ]}
            onClose={() => setAnchor(null)}
          />
        )}
      </>
    );
  }
  const press = async (key: string) => {
    await act(async () => {
      document.activeElement?.dispatchEvent(new window.KeyboardEvent("keydown", { key, bubbles: true }));
      if (key === "ArrowRight") await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    });
  };

  await render(<Page />);
  await click("#actions");
  expect(document.activeElement?.textContent).toBe("Move to section");
  await press("ArrowDown");
  expect(document.activeElement?.textContent).toBe("Archive");
  await press("ArrowUp");
  await press("ArrowRight");
  expect(document.querySelector('[aria-current="true"]')?.textContent).toBe("Dialog");
  expect(document.activeElement?.textContent).toBe("Chauffeur");
  await press("ArrowLeft");
  expect(document.activeElement?.textContent).toBe("Move to section");
  await press("ArrowRight");
  await click('[role="menuitem"]:focus');
  expect(document.querySelector("output")?.textContent).toBe("Chauffeur");
  expect(document.querySelector('[role="menu"]')).toBeNull();
  expect(document.activeElement?.id).toBe("actions");
});
