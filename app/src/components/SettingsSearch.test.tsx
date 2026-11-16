import { expect, spyOn } from "bun:test";
import { act } from "react";
import { domTest } from "@/test/dom";

const file = import.meta.path;

domTest("settings search opens and highlights an option and hides stale content for no results", file, async ({ render, fill, click, until }) => {
  const { client, useApp } = await import("@/state/app");
  const { SettingsDialog } = await import("./SettingsDialog");
  useApp.setState({ settingsOpen: true, settingsSection: "general", identity: null });
  const call = spyOn(client, "call").mockResolvedValue({
    start_at_login: false, daemon_port: 41000, file: "/tmp/settings.toml", error_reports_available: false,
    appearance: { interface_font: "", code_font: "", code_size: 0, terminal_font: "", terminal_size: 0, terminal_theme: "", palette: null, palette_error: null, theme_interface: false },
  });
  const scroll = spyOn(HTMLElement.prototype, "scrollIntoView").mockImplementation(() => {});
  try {
    await render(<SettingsDialog />);
    await until(() => !!document.querySelector('[data-testid="start-at-login"]'));
    await fill('[aria-label="Search settings"]', "terminal font");
    expect(document.querySelector('[data-testid="settings-content"]')?.textContent).toContain("Preferences › Appearance");
    expect(document.querySelector('[data-testid="start-at-login"]')).toBeNull();
    await click('[data-testid="settings-result-font-terminal"]');
    expect(document.querySelector('[data-testid="font-terminal"]')?.hasAttribute("data-search-match")).toBe(true);
    expect(document.activeElement?.getAttribute("data-testid")).toBe("font-terminal");
    expect(scroll).toHaveBeenCalled();
    await fill('[aria-label="Search settings"]', "nothing-matches-this");
    expect(document.querySelector('[role="status"]')?.textContent).toContain("No matching settings");
    expect(document.querySelector('[data-testid="font-terminal"]')).toBeNull();
    await fill('[aria-label="Search settings"]', "slack");
    expect(document.querySelector('[role="status"]')?.textContent).toContain("No matching settings");
    await act(async () => { useApp.setState({ settingsOpen: false }); });
    await act(async () => { useApp.setState({ settingsOpen: true }); });
    expect(document.querySelector<HTMLInputElement>('[aria-label="Search settings"]')?.value).toBe("");
  } finally { call.mockRestore(); scroll.mockRestore(); }
});
