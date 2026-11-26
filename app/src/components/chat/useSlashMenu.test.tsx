import { expect, spyOn } from "bun:test";
import type { SlashCommand } from "@chauffeur/protocol";
import { act } from "react";
import { domTest } from "@/test/dom";

domTest("the / menu inserts its command with Tab or ↵, never with ⇧Tab", import.meta.path, async ({ render, until }) => {
  const { client } = await import("@/state/app");
  const { useSlashMenu } = await import("./useSlashMenu");
  spyOn(client, "call").mockImplementation(async () => ({ commands: [{ name: "compact", description: "Compact", argument_hint: null }] }));
  const inserted: string[] = [];
  let keys: ReturnType<typeof useSlashMenu>["onKeyDown"] = () => false;
  function Composer() {
    const slash = useSlashMenu("/com", { repos: ["shop-web"], agent: "claude" }, (command: SlashCommand) => inserted.push(command.name));
    keys = slash.onKeyDown;
    return slash.menu;
  }
  await render(<Composer />);
  await until(() => document.querySelector("[data-testid=slash-menu] [role=option]") !== null);
  const press = (key: string, shiftKey = false) => {
    let taken = false;
    act(() => {
      taken = keys({ key, shiftKey, ctrlKey: false, metaKey: false, altKey: false, isComposing: false, preventDefault: () => {} });
    });
    return taken;
  };
  expect(press("Tab", true)).toBe(false);
  expect(inserted).toEqual([]);
  expect(press("Tab")).toBe(true);
  expect(press("Enter")).toBe(true);
  expect(inserted).toEqual(["compact", "compact"]);
});
