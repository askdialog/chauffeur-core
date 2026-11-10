import { describe, expect, test } from "bun:test";
import { MAX_INLINE_BYTES, userContent } from "../src/agents/claude/content.ts";
import { ClaudeSession } from "../src/agents/claude/session.ts";
import { userInputs } from "../src/agents/codex/options.ts";
import { CodexSession } from "../src/agents/codex/session.ts";
import type { Attachment, SessionStartParams } from "../src/protocol.ts";
import { codex, FakeAppServer, HOOK } from "./support/codex-fakes.ts";
import { FakeHost, FakeQuery, flush } from "./support/fakes.ts";

const SHOT: Attachment = {
  file: "0192-shot.png",
  name: "Screenshot.png",
  media_type: "image/png",
  size: 4,
  path: "/ws/.chauffeur/attachments/0192-shot.png",
};

describe("images for Claude", () => {
  test("a message without images stays plain text", () => {
    expect(userContent("hi", [], "/ws")).toBe("hi");
  });

  test("images come first as base64 blocks, the text names their files", () => {
    const content = userContent("look", [SHOT], "/ws", () => Buffer.from("png!"));
    expect(content).toEqual([
      { type: "image", source: { type: "base64", media_type: "image/png", data: Buffer.from("png!").toString("base64") } },
      { type: "text", text: "look\n\n[The attached image is shown above; it is also saved at .chauffeur/attachments/0192-shot.png]" },
    ]);
  });

  test("an image too large to inline is left for the Read tool", () => {
    const big = { ...SHOT, size: MAX_INLINE_BYTES + 1 };
    const content = userContent("", [big], "/ws", () => {
      throw new Error("not read");
    });
    expect(content).toEqual([
      { type: "text", text: "[An image is attached at .chauffeur/attachments/0192-shot.png: too large to show here, read it with the Read tool]" },
    ]);
  });

  test("the first prompt's images go with it", async () => {
    let query: FakeQuery | undefined;
    const session = new ClaudeSession(
      {
        chat_id: "c",
        workspace_id: "w",
        agent: "claude",
        cwd: "/nowhere",
        additional_directories: [],
        fast: false,
        permission_mode: "default",
        read_only: false,
        prompt_suggestions: false,
        env: {},
        prompt: "what is this?",
        prompt_attachments: [{ ...SHOT, path: "/nowhere/missing.png" }],
      },
      new FakeHost(),
      { query: ({ prompt, options }) => (query = new FakeQuery(prompt, options)), resolveExecutable: () => undefined, baseEnv: {} },
    );
    await session.start();
    await query?.waitForSent(1);
    // The file does not exist: the image is named for the Read tool rather than dropped.
    expect(query?.sent[0]?.message.content).toEqual([
      { type: "text", text: "what is this?\n\n[An image is attached at missing.png: too large to show here, read it with the Read tool]" },
    ]);
    await session.stop();
  });
});

const ZIP: Attachment = {
  file: "0193-logs.zip",
  name: "logs.zip",
  media_type: "application/zip",
  size: 2048,
  path: "/ws/.chauffeur/attachments/0193-logs.zip",
};
const ZIP_NOTE = '[The user attached the file "logs.zip" (application/zip); it is saved at .chauffeur/attachments/0193-logs.zip]';

describe("other files", () => {
  test("Claude is told where a file is, never sent it as an image", () => {
    const content = userContent("what failed?", [SHOT, ZIP], "/ws", (path) => {
      if (path === ZIP.path) throw new Error("a file is not read");
      return Buffer.from("png!");
    });
    expect(content).toEqual([
      { type: "image", source: { type: "base64", media_type: "image/png", data: Buffer.from("png!").toString("base64") } },
      {
        type: "text",
        text: `what failed?\n\n[The attached image is shown above; it is also saved at .chauffeur/attachments/0192-shot.png]\n${ZIP_NOTE}`,
      },
    ]);
  });

  test("Codex gets the file's path in the text, and no localImage for it", () => {
    expect(userInputs("", [ZIP], "default", "/ws")).toEqual([{ type: "text", text: ZIP_NOTE, text_elements: [] }]);
  });
});

describe("images for Codex", () => {
  test("each image is a localImage input after the text", () => {
    expect(userInputs("look", [SHOT], "default", "/ws")).toEqual([
      { type: "text", text: "look\n\n[The attached image is shown above; it is also saved at .chauffeur/attachments/0192-shot.png]", text_elements: [] },
      { type: "localImage", path: SHOT.path },
    ]);
    expect(userInputs("", [SHOT], "default", "/ws")).toEqual([
      { type: "text", text: "[The attached image is shown above; it is also saved at .chauffeur/attachments/0192-shot.png]", text_elements: [] },
      { type: "localImage", path: SHOT.path },
    ]);
  });

  test("a message with images starts a turn, and steers a running one, with them", async () => {
    const BASE: SessionStartParams = {
      chat_id: "c",
      workspace_id: "w",
      agent: "codex",
      cwd: "/ws",
      additional_directories: [],
      fast: false,
      prompt_attachments: [],
      permission_mode: "default",
      read_only: false,
      prompt_suggestions: false,
      env: {},
    };
    let server: FakeAppServer | undefined;
    const session = new CodexSession(BASE, new FakeHost(), {
      spawn: (options) => (server = new FakeAppServer(options)),
      resolveExecutable: () => "/usr/local/bin/codex",
      baseEnv: {},
      hookCommand: () => HOOK,
      initTimeoutMs: 500,
    });
    await session.start();
    session.send("look", [SHOT]);
    await flush();
    expect(server?.sent("turn/start")[0].input).toContainEqual({ type: "localImage", path: SHOT.path });
    server?.push(codex.turnStarted("turn-1"));
    await flush();
    session.send("and this", [SHOT]);
    await flush();
    expect(server?.sent("turn/steer")[0].input).toContainEqual({ type: "localImage", path: SHOT.path });
    await session.stop();
  });
});
