# 0009. Settings window, appearance and defaults

- **Date:** 2026-10-15 (also 2026-10-16, 2026-10-20, 2026-10-28, 2026-10-29)
- **Status:** accepted

## Context

Users should not have to edit `settings.toml` or open a terminal to configure Chauffeur, and Chauffeur should not assume Dialog's organization, repos or cloud project.

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 45:

> As a developer, I want to change in the app's settings window (⌘,) the repos a workspace starts with, the branch prefix, the folders with my clones, the default model, Linear (its key, whether it is used, the move to In Progress) and starting Chauffeur at login, so that I never edit settings.toml or open a terminal for them (decided with Justin 2026-10-15).

From `SPEC.md`, Implementation decisions:

> Settings keeps the same window height across sections (600 px, capped at 80% of the viewport); long sections scroll independently of the section navigation and header (asked by Justin 2026-10-20).

From `SPEC.md`, Implementation decisions:

> The macOS “About Chauffeur” panel keeps the native app icon and running version, identifies Chauffeur as “By Dialog”, and presents its purpose, Dialog credits and repository address (asked by Justin 2026-10-20).

From `SPEC.md`, Config:

> Outside the Office 3D and its collaboration services, defaults are independent of Dialog: no GitHub organization, preselected repos, clone roots or Google Cloud project. Teams configure these explicitly in `~/.chauffeur/settings.toml`; a repository's settings file remains for its worktree scripts and is not a user-settings import (decided with Justin 2026-10-29).

From `SPEC.md`, Config:

> User settings in `~/.chauffeur/settings.toml`: GitHub organization, default repo set, branch prefix, clone folders, Linear's switches, children limit, ports, delays, keeping the machine awake. The daemon reads it again within a second of a change by hand (a version that does not parse leaves the last valid one in force, and the settings window says so), and writes it for the settings window with `toml_edit`, keeping its comments and layout. Every key of the file can be changed in the settings window (⌘,) too, out-of-range values refused (decided with Justin 2026-10-16); the daemon's port applies at its next start, the rest at once. "Open" writes it first, when missing, with every key commented out at its default.

From `SPEC.md`, Config:

> Settings → LLMs lists each favorite with its own reasoning and Fast controls, addition, removal and ordering; making a favorite the default moves it first. Agents keeps idle-session and prompt-suggestion settings (decided with Justin 2026-10-28).

From `SPEC.md`, Fonts:

> **Fonts (decided with Justin 2026-10-16 after a user's feedback).** Settings › Appearance picks, by name, the font of the interface, of the code (the Files editor, the Diff and code in chats) and of the terminal, with a size for the code and the terminal (the interface's size is the app's zoom); kept in `[appearance]` of `settings.toml`, applied at once, and again when the app comes back in front after a change made by hand. A font not installed falls back to the app's own; a name may not carry anything but letters, digits, spaces, commas, quotes, dashes and dots.

From `SPEC.md`, Colours:

> **Colours (decided with Justin 2026-10-16 after a user's feedback).** `terminal_theme` in `[appearance]` (Settings › Appearance) names a colour theme file the user's own tools write: pywal JSON, kitty, Ghostty, foot or Alacritty, the formats Dracula and most theme collections ship and wallust's templates write (foot added after a user showed their wallust setup). The terminal takes its background, foreground, cursor, selection and 16 ANSI colours; the code (Files editor, Diff, code in chats) its background and foreground, and syntax colours from its ANSI colours. The daemon reads the file on every `settings.get`, so the app, which asks again every 5 s while it is in front, follows a theme rewritten when the wallpaper changes. A file that is not a whole palette is reported, and the app keeps its own colours.

## Consequences

Every key of `settings.toml` is editable in the settings window, and a change by hand applies within a second. Teams set their organization, repos and clone folders explicitly; the Office and its collaboration services kept their Dialog configuration until 2026-11-05, when Core dropped every default pointing at Dialog's services ([core-open-source-without-dialog-defaults](core-open-source-without-dialog-defaults.md)).
