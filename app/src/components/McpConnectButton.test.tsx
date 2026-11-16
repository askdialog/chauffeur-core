import { expect, spyOn } from "bun:test";
import type { ChatInfo, McpServer } from "@chauffeur/protocol";
import { act } from "react";
import { deferred, domTest } from "@/test/dom";

const file = import.meta.path;
const server = (status: McpServer["status"]): McpServer => ({
  name: "slack", status, error: null, failed_call: false, missing_env: [], uses_env: [], declared_in: null,
});
const chat = (status: McpServer["status"]): ChatInfo => ({
  id: "chat", workspace_id: "workspace", kind: "chat", agent: "claude", model: null, effort: null,
  fast: false, permission_mode: "auto", title: "Support", status: "idle", agent_session_id: "session",
  created_at: "2026-11-16T10:00:00Z", running_tasks: 0, closed: false, last_activity_at: null,
  mcp_servers: [server(status)],
});

domTest("a direct reconnect shares its progress and keeps the composer panel open until confirmed", file, async ({ render, click, until }) => {
  const { client, connectMcp, useApp } = await import("@/state/app");
  const { useToasts } = await import("@/state/toasts");
  const { McpConnectButton } = await import("./McpConnectButton");
  const { McpServers } = await import("./chat/McpServers");
  const initial = chat("failed");
  useApp.setState({ chats: { chat: initial }, connectingMcp: {} });
  const reply = deferred<{ auth_url: null }>();
  const call = spyOn(client, "call").mockImplementation(() => reply.promise);
  function Screen() {
    const current = useApp((s) => s.chats.chat);
    return <><McpConnectButton chat={current} server={current.mcp_servers[0]} /><McpServers chat={current} /></>;
  }
  try {
    await render(<Screen />);
    await click('[data-testid="mcp-servers"]');
    await click('[aria-label="Reconnect slack"]');
    expect(document.querySelectorAll('button[aria-label="Reconnect slack"]:disabled')).toHaveLength(2);
    await connectMcp(initial, "slack");
    expect(call).toHaveBeenCalledTimes(1);
    await act(async () => {
      useApp.setState({ chats: { chat: chat("pending") } });
      reply.resolve({ auth_url: null });
    });
    expect(useToasts.getState().toasts.some((t) => t.message.includes("connected again"))).toBe(false);
    expect(useApp.getState().connectingMcp.chat).toEqual(["slack"]);
    await act(async () => { useApp.setState({ chats: { chat: chat("connected") } }); });
    await until(() => useApp.getState().connectingMcp.chat === undefined);
    expect(document.querySelector('[data-testid="mcp-panel"]')?.textContent).toContain("Connected");
    expect(useToasts.getState().toasts.at(-1)?.message).toBe("slack is connected again");
  } finally { call.mockRestore(); }
});

domTest("an OAuth page waits for the session to confirm the login", file, async () => {
  const { client, connectMcp, useApp } = await import("@/state/app");
  const { useToasts } = await import("@/state/toasts");
  const initial = chat("needs_auth");
  useApp.setState({ chats: { chat: initial } });
  const call = spyOn(client, "call").mockResolvedValue({ auth_url: "https://example.com/oauth" });
  const open = spyOn(window, "open").mockReturnValue(null);
  try {
    const attempt = connectMcp(initial, "slack");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(open).toHaveBeenCalledWith("https://example.com/oauth", "_blank", "noopener");
    expect(useApp.getState().connectingMcp.chat).toEqual(["slack"]);
    expect(useToasts.getState().toasts.at(-1)?.message).toContain("Finish logging in");
    useApp.setState({ chats: { chat: chat("connected") } });
    await attempt;
    expect(useToasts.getState().toasts.at(-1)?.message).toBe("slack is connected again");
  } finally { call.mockRestore(); open.mockRestore(); }
});

domTest("a reconnect that fails reports failure and allows another attempt", file, async () => {
  const { client, connectMcp, useApp } = await import("@/state/app");
  const { useToasts } = await import("@/state/toasts");
  useApp.setState({ chats: { chat: chat("pending") } });
  const call = spyOn(client, "call").mockResolvedValue({ auth_url: null });
  try {
    const attempt = connectMcp(chat("failed"), "slack");
    await new Promise((resolve) => setTimeout(resolve, 0));
    useApp.setState({ chats: { chat: chat("failed") } });
    await attempt;
    expect(useToasts.getState().toasts.at(-1)).toMatchObject({ level: "error", message: "slack still does not connect: Failed to connect" });
    expect(useApp.getState().connectingMcp.chat).toBeUndefined();
  } finally { call.mockRestore(); }
});

domTest("a missing server never confirms success and a closed chat ends the wait", file, async () => {
  const { client, connectMcp, useApp } = await import("@/state/app");
  const { useToasts } = await import("@/state/toasts");
  useApp.setState({ chats: { chat: { ...chat("failed"), mcp_servers: [] } } });
  const call = spyOn(client, "call").mockResolvedValue({ auth_url: null });
  try {
    const attempt = connectMcp(chat("failed"), "slack");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useApp.getState().connectingMcp.chat).toEqual(["slack"]);
    useApp.setState((s) => ({ chats: { ...s.chats, other: { ...chat("connected"), id: "other" } } }));
    expect(useApp.getState().connectingMcp.chat).toEqual(["slack"]);
    useApp.setState({ chats: { chat: { ...chat("failed"), closed: true } } });
    await attempt;
    expect(useToasts.getState().toasts.at(-1)?.level).toBe("error");
    expect(useApp.getState().connectingMcp.chat).toBeUndefined();
  } finally { call.mockRestore(); }
});

domTest("a native login without a browser URL is followed until connected", file, async () => {
  const { client, connectMcp, useApp } = await import("@/state/app");
  const { useToasts } = await import("@/state/toasts");
  const initial = chat("needs_auth");
  useApp.setState({ chats: { chat: initial } });
  const call = spyOn(client, "call").mockResolvedValue({ auth_url: null });
  try {
    const attempt = connectMcp(initial, "slack");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(useApp.getState().connectingMcp.chat).toEqual(["slack"]);
    expect(useToasts.getState().toasts).toHaveLength(0);
    useApp.setState({ chats: { chat: chat("connected") } });
    await attempt;
    expect(useToasts.getState().toasts.at(-1)?.message).toBe("slack is connected again");
  } finally { call.mockRestore(); }
});
