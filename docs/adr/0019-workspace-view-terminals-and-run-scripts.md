# 0019. Workspace view, terminal tabs and run scripts

- **Date:** 2026-10-18 (also 2026-10-20, 2026-10-26)
- **Status:** accepted

## Context

Users want a terminal or a diff beside the chat, terminal tabs that behave like chat tabs, and the workspace's local stack started once, visibly, whoever starts it.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 26:

> The workspace view splits as in VS Code: one view tab (Terminal, Diff, Files, Processes or Chat) beside the main pane, on the right or below, from the tab's right-click menu, by dragging the tab to the view's right or bottom edge (back onto the tab bar to return it), or with ⌘\; a resizable divider, a single pane while the window is too small, the layout kept per workspace, and a terminal moved between panes keeps its shell and scrollback (decided with Justin 2026-10-18). A terminal's tab has a right-click menu, as a chat's: Rename (or double-click; empty gives back its title, the repo or workspace it started in; the daemon keeps the name, control and bidi characters left out, for as long as the terminal lives, `terminal.rename`), Close, Close the others, Open in the terminal app (the default app when it is a terminal, else the first found) and Copy the path, both of the folder its shell is in now, read from the shell's process (`terminal.cwd`; where it started once the shell exited or its folder was removed; `workspace.open` takes the terminal and the daemon resolves its folder) (asked by a user 2026-10-25, decided with Justin 2026-10-26). A closed terminal cannot be reopened: closing one whose shell runs a command, from its tab, its menu, Close the others (which names those that run one) or a middle click, asks first, as a chat asks before stopping its agent (asked by Justin 2026-10-26).

From `SPEC.md`, story 27:

> As a developer, I want to start and stop each repo's run scripts and read their output, so that I run the workspace's local stack. Agents start the local stack the same way: the workspace's `CLAUDE.md` and `AGENTS.md` tell them to use a repo's run scripts (`chauffeur ps`, `chauffeur run`, `chauffeur stop`, the output from `chauffeur ps --json`) rather than a background command of their own, so that it shows in Processes, outlives the agent's session and is not started twice; a repo without run scripts they run themselves (asked by a user 2026-10-19, decided with Justin 2026-10-20).

## Consequences

The layout is kept per workspace; a closed terminal cannot be reopened, so closing one that runs a command asks first. Agents start the stack through run scripts (`chauffeur run`) so that it shows in Processes.
