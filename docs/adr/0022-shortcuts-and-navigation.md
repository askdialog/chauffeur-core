# 0022. Keyboard shortcuts, custom bindings and workspace history

- **Date:** 2026-10-18 (also 2026-10-26, 2026-10-29)
- **Status:** accepted

## Context

Shortcuts must work on AZERTY and on Linux, not collide with macOS's or the terminal's, and be remappable.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, Shortcuts:

> **Shortcuts.** ⌘1–9 select the sidebar's workspaces as shown (a folded section's skipped) and ⌃⌘1–9 the model picker's favorites, read from the key's place (`event.code`): on AZERTY the digits need Shift, and ⌘⇧3–5 are macOS's screenshots (2026-10-15). ⌘\ opens or closes the workspace view's side pane, Terminal on the right at first (decided with Justin 2026-10-18). ⌘W closes the chat tab shown and ⇧⌘T reopens the last one closed, as a browser's tabs; the window closes with ⇧⌘W (2026-10-18). In the Terminal view, ⌘T opens a terminal and ⌘W closes the one shown (the tab after it shows; one whose shell runs a command asks first, a closed terminal being lost), Ctrl+Shift+T and Ctrl+Shift+W on Linux, where Ctrl+T and Ctrl+W stay the shell's; there the macOS File menu reads New Terminal and Close Terminal, on those keys, and Reopen Closed Chat, which still reopens a chat, shows no keys (asked by Justin 2026-10-26: the menu says what its keys do). ⌃\` (Ctrl+\` on Linux, read from the key's place: ⌃² on AZERTY) shows the Terminal view, and from it the Chat view; ⌥⌘1–5 (Alt+1–5 on Linux, where Ctrl+Alt+1–9 stay the model picker's favorites; AltGr still types) show Chat, Diff, Files, Terminal and Processes, as clicking their tab does (a view in the side pane is focused there, the pane stays as it is). They act from the terminal (readline's Alt+digit is given up), the composer and the editors too (asked by a user 2026-10-25; Alt+1–5 on Linux decided with Justin 2026-10-26).

From `SPEC.md`, Keys:

> The app's shortcuts (one registry in `app/src/lib/shortcuts.ts`: id, default keys per platform, what and where) take other keys from `[keyboard.bindings]` (`"chat.new" = "⇧⌘J"`, written the macOS way, ⌘ being Ctrl on Linux and ⌃ Ctrl+Alt), one binding replacing a shortcut's defaults, which may take ⌥ (Alt on Linux, where ⌥⌘ is ⌃'s Ctrl+Alt and Alt alone may be a binding's modifier, as in ⌥1 for Alt+1; decided with Justin 2026-10-26); the ids, the protocol's `ShortcutId`, are part of the `settings.toml` contract (decided with Justin 2026-10-26). Settings › Keyboard records them (the keys shown as they are held, Esc cancels, ⌫ the default, a click elsewhere or Tab ends it), says why it refuses a press, flags two shortcuts on the same keys, keys that other handlers read included (find in a chat, ⌘[ and ⌘] between workspaces, ⌘S in the Files editor, ⌘↵ in the composer, ⇧⌘W), not those of the Chat and the Terminal views' tabs, which share ⌘T and ⌘W, and resets them all. No shortcut takes a reserved key: the Edit menu's (⌘A, ⌘C, ⌘V, ⌘X, ⌘Z, ⇧⌘Z), macOS's own (⌘Q, ⌘H, ⌘M, ⌃⌘F), and the terminal's that a running program needs (⌘C, ⌘D, ⌘Z, Ctrl outside macOS; ⌃C, ⌃D, ⌃Z on macOS); a key is a printable ASCII character or a named key. The daemon leaves out an unknown id, a value that is not keys in quotes, keys that are not a binding or are reserved, with a warning in its log and in Settings, and the rest of the file applies; it refuses them from the app. The rows of digits (⌘1–9, ⌃⌘1–9) keep their keys. On Linux the terminal keeps the shell's Ctrl keys from the defaults (those that switch tabs and views and open or close terminals excepted), not from keys the user set: those are the app's there too. The macOS menus show and catch the keys in force (the terminals' New and Close while the Terminal view is in use), and none while Settings records keys; their predefined items (Quit, Hide, Minimize, the Edit menu, Full Screen) keep theirs (decided with Justin 2026-10-26).

From `SPEC.md`, Shortcuts:

> The workspace header has a back and a forward arrow (⌘[ and ⌘]), as a browser has: back returns to the workspace shown before this one, forward goes there again. Only changes of workspace count, not of chat tab; archived workspaces are passed over; the history is kept in memory, not across launches, and is not used in the Office (decided with Justin 2026-10-29).

## Consequences

The shortcut ids (`ShortcutId` in the protocol) are part of the `settings.toml` contract; renaming one breaks users' bindings.
