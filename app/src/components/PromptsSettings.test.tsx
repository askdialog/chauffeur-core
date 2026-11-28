import { expect, spyOn } from "bun:test";
import type { Prompt } from "@chauffeur/protocol";
import { domTest } from "@/test/dom";

const file = import.meta.path;

const names = () => [...document.querySelectorAll<HTMLInputElement>('[data-testid="prompt-name"]')].map((input) => input.value);
const within = (index: number, testId: string) =>
  document.querySelectorAll('[data-testid="prompt"]')[index]?.querySelector<HTMLElement>(`[data-testid="${testId}"]`) ?? null;

domTest("Settings › Prompts adds, renames, edits and deletes prompts through the daemon, each error beside its field", file, async ({ render, click, fill, until }) => {
  const { RpcCallError } = await import("@/lib/rpc");
  const { client } = await import("@/state/app");
  const { PromptsSettings } = await import("./PromptsSettings");
  const sort = (list: Prompt[]) => [...list].sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
  let prompts: Prompt[] = [{ name: "agreed", text: "Agreed, carry on" }, { name: "Compact", text: "Compact:\n- keep the decisions" }];
  let saves = 0;
  const call = spyOn(client, "call").mockImplementation(async (method, params) => {
    if (method === "prompts.list") return prompts;
    if (method === "prompts.save" && "text" in params && "name" in params) {
      saves += 1;
      if (params.name === "") throw new RpcCallError("prompts.save", { code: 400, message: "A prompt needs a name", data: { field: "name" } });
      if (params.text === "disk full") throw new RpcCallError("prompts.save", { code: -32603, message: "No space left on device" });
      // As the daemon: one trailing newline is not part of the prompt.
      prompts = sort([...prompts.filter((p) => p.name !== (params.previous ?? params.name)), { name: params.name, text: params.text.replace(/\n$/, "") }]);
      return prompts;
    }
    if (method === "prompts.delete" && "name" in params) {
      prompts = prompts.filter((p) => p.name !== params.name);
      return prompts;
    }
    throw new Error(`Unexpected RPC: ${method}`);
  });
  let answer = false;
  const asked: string[] = [];
  window.confirm = (message?: string) => { asked.push(message ?? ""); return answer; };
  const focused = () => document.activeElement?.closest<HTMLElement>('[data-testid="prompt"]')?.dataset.prompt;
  try {
    await render(<PromptsSettings />);
    await until(() => names().length === 2);
    expect(names()).toEqual(["agreed", "Compact"]);
    expect((within(1, "prompt-text") as HTMLTextAreaElement | null)?.value).toBe("Compact:\n- keep the decisions");

    // New: an empty name is the daemon's to refuse, beside the name; typing clears it.
    await click('[data-testid="prompt-new"]');
    expect(names()).toEqual(["", "agreed", "Compact"]);
    await fill('[data-testid="prompt-text"]', "Yes, go ahead");
    await click('[data-testid="prompt-save"]');
    await until(() => !!within(0, "prompt-name-error"));
    expect(within(0, "prompt-name-error")?.textContent).toBe("A prompt needs a name");
    expect(within(0, "prompt-save-error")).toBeNull();
    await fill('[data-testid="prompt-name"]', "y");
    expect(within(0, "prompt-name-error")).toBeNull();
    // A new prompt never takes an existing prompt's name, which would replace its text, even one
    // made since the section opened.
    prompts = sort([...prompts, { name: "by hand", text: "Made meanwhile" }]);
    for (const taken of ["agreed", "by hand"]) {
      await fill('[data-testid="prompt-name"]', taken);
      await click('[data-testid="prompt-save"]');
      await until(() => !!within(0, "prompt-name-error"));
      expect(within(0, "prompt-name-error")?.textContent).toContain(`already named "${taken}"`);
    }
    expect(saves).toBe(1);
    await fill('[data-testid="prompt-name"]', "yes");
    await click('[data-testid="prompt-save"]');
    await until(() => names().length === 4 && !names().includes(""));
    expect(names()).toEqual(["agreed", "by hand", "Compact", "yes"]);
    expect(focused()).toBe("yes");

    // Renaming an existing prompt names the one it was, and keeps the keyboard on it.
    await fill('[data-testid="prompt-name"]', "carry on");
    await click('[data-testid="prompt-save"]');
    await until(() => names().includes("carry on"));
    expect(names()).toEqual(["by hand", "carry on", "Compact", "yes"]);
    expect(focused()).toBe("carry on");

    // Saved with a trailing newline, the text reads as the daemon keeps it: nothing left to save.
    await fill('[data-testid="prompt"]:nth-child(1) [data-testid="prompt-text"]', "Made meanwhile\n");
    await click('[data-testid="prompt"]:nth-child(1) [data-testid="prompt-save"]');
    await until(() => (within(0, "prompt-save") as HTMLButtonElement | null)?.disabled === true);
    expect((within(0, "prompt-text") as HTMLTextAreaElement | null)?.value).toBe("Made meanwhile");

    // An error about something other than the name shows beside Save.
    await fill('[data-testid="prompt"]:nth-child(3) [data-testid="prompt-text"]', "disk full");
    await click('[data-testid="prompt"]:nth-child(3) [data-testid="prompt-save"]');
    await until(() => !!within(2, "prompt-save-error"));
    expect(within(2, "prompt-save-error")?.textContent).toBe("No space left on device");
    expect(within(2, "prompt-name-error")).toBeNull();

    // Delete asks first: declined, nothing goes.
    await click('[data-testid="prompt"]:nth-child(4) [data-testid="prompt-delete"]');
    expect(asked).toEqual(['Delete the prompt "yes"?']);
    expect(names()).toHaveLength(4);
    answer = true;
    await click('[data-testid="prompt"]:nth-child(4) [data-testid="prompt-delete"]');
    await until(() => names().length === 3);
    expect(names()).toEqual(["by hand", "carry on", "Compact"]);
    expect(document.activeElement?.getAttribute("data-testid")).toBe("prompt-new");
    // The other prompt kept the text being edited.
    expect((within(2, "prompt-text") as HTMLTextAreaElement | null)?.value).toBe("disk full");
  } finally {
    await render(null);
    call.mockRestore();
  }
});

domTest("Settings › Prompts reads the folder again each time it opens, and says when it is empty", file, async ({ render, until }) => {
  const { client } = await import("@/state/app");
  const { PromptsSettings } = await import("./PromptsSettings");
  let prompts: Prompt[] = [];
  const call = spyOn(client, "call").mockImplementation(async (method) => {
    if (method === "prompts.list") return prompts;
    throw new Error(`Unexpected RPC: ${method}`);
  });
  try {
    await render(<PromptsSettings />);
    await until(() => !!document.querySelector('[data-testid="prompts-empty"]'));
    prompts = [{ name: "by hand", text: "Edited by hand" }];
    await render(null);
    await render(<PromptsSettings />);
    await until(() => names().length === 1);
    expect((within(0, "prompt-text") as HTMLTextAreaElement | null)?.value).toBe("Edited by hand");
    expect(document.querySelector('[data-testid="prompts-empty"]')).toBeNull();
  } finally {
    await render(null);
    call.mockRestore();
  }
});

domTest("openSettings with the prompts section opens Settings › Prompts", file, async ({ render, until }) => {
  const { client, openSettings, useApp } = await import("@/state/app");
  const { SettingsDialog } = await import("./SettingsDialog");
  useApp.setState({ identity: null });
  const call = spyOn(client, "call").mockImplementation(async (method) => {
    if (method === "prompts.list") return [];
    if (method === "settings.get") return { file_error: null };
    throw new Error(`Unexpected RPC: ${method}`);
  });
  try {
    await render(<SettingsDialog />);
    openSettings(true, "prompts");
    await until(() => !!document.querySelector('[data-testid="prompts-settings"]'));
    expect(document.querySelector('[data-testid="settings-prompts"]')?.getAttribute("aria-current")).toBe("page");
  } finally {
    await render(null);
    call.mockRestore();
  }
});

domTest("a prompt's fields stay read-only while its save is on its way, so nothing typed then is replaced", file, async ({ render, click, fill, until }) => {
  const { client } = await import("@/state/app");
  const { deferred } = await import("@/test/dom");
  const { PromptsSettings } = await import("./PromptsSettings");
  const reply = deferred<Prompt[]>();
  const call = spyOn(client, "call").mockImplementation(async (method) => {
    if (method === "prompts.list") return [{ name: "yes", text: "Yes" }];
    if (method === "prompts.save") return reply.promise;
    throw new Error(`Unexpected RPC: ${method}`);
  });
  const field = (testId: string) => within(0, testId) as HTMLInputElement | HTMLTextAreaElement | null;
  try {
    await render(<PromptsSettings />);
    await until(() => names().length === 1);
    expect(field("prompt-text")?.readOnly).toBe(false);
    await fill('[data-testid="prompt-text"]', "Yes, go ahead");
    await click('[data-testid="prompt-save"]');
    expect(field("prompt-name")?.readOnly).toBe(true);
    expect(field("prompt-text")?.readOnly).toBe(true);
    reply.resolve([{ name: "yes", text: "Yes, go ahead" }]);
    await until(() => field("prompt-text")?.readOnly === false);
    expect(field("prompt-text")?.value).toBe("Yes, go ahead");
  } finally {
    await render(null);
    call.mockRestore();
  }
});
