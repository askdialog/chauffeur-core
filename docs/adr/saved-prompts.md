# The user's own prompts, inserted from the composer with ⇧Tab

- **Date:** 2026-11-23
- **Status:** proposed, awaiting Justin

## Context

The user sends some messages over and over, either short replies ("Yes, go ahead", "Agreed, carry on") or long instructions written once with care (how to compact the conversation, say), and types them again each time. Claude's prompt suggestion ([0012](0012-composer.md)) guesses one message and does not know the user's wording. A file in `.claude/commands` shows in the `/` menu (story 41), but only in a Claude chat, it is sent as a command for Claude to expand rather than as text the user can edit first, and it is written by hand outside Chauffeur. Asked by Théo 2026-11-23; awaiting Justin's approval, since it adds a key to the composer and a folder to `~/.chauffeur`.

## Decision

Asked by Théo 2026-11-23:

- **The user's own prompts.** A prompt has a name and a text, short or long. The user creates, edits and deletes their prompts in Settings › Prompts. Prompts are global: the same in every workspace and every chat, with no prompts belonging to a repo or a workspace.
- **⇧Tab opens them.** In a chat's composer, ⇧Tab opens a menu of the prompts above the composer, whatever the chat's agent (Claude, Codex, Mistral Vibe) and whatever the composer holds. Typing filters the menu by name and text, ↑↓ move, ↵ inserts the chosen prompt's text at the caret, and Esc closes the menu. Nothing is sent: the user reads, edits and sends the message as any other. Plain Tab keeps taking the prompt suggestion.
- **No insert-and-send.** No key inserts a prompt and sends it at once; ⌘↵ keeps its meaning in the composer (queue while the agent works).
- **One Markdown file per prompt** in `~/.chauffeur/prompts/`: the file's name, without `.md`, is the prompt's name, and its content is the text as typed. The menu lists the prompts in alphabetical order.
- **The text goes as typed.** A prompt starting with `/compact …` runs Claude Code's command in a Claude chat, as it would if typed; in a Codex or Vibe chat it is an ordinary message.

## Consequences

- The daemon writes the prompts, as it writes everything under `~/.chauffeur`: `prompts.list` (each prompt's name and text, read from the folder on every call, so a file edited by hand shows the next time the menu opens), `prompts.save` (name, text, and the previous name for a rename) and `prompts.delete`, callable by the user's clients, not by agents. The folder is created on the first save.
- A name is not empty, holds no `/`, does not start with `.`, and is unique ignoring case (macOS's file system ignores it). A file that is not `.md`, or not UTF-8, is left out of the list.
- The app adds a prompts menu beside the `/` menu of the chat's composer (`useSlashMenu`). The `/` menu takes Tab without Shift only, so that ⇧Tab always opens the prompts. With no prompt yet, the menu says so and opens Settings › Prompts.
- ⇧Tab is fixed, as the composer's other keys (Tab, Esc, ⌘↵) are: it is not a `ShortcutId` and `[keyboard.bindings]` cannot change it.
- Settings › Prompts is in the "Work" group: the list, and for each prompt its name and a multi-line text field. Settings search finds it under "prompts", "snippets" and "saved replies".
- Left for later: the new-workspace dialog's first prompt, a `chauffeur prompts` command, the mobile companion, variables in a prompt, prompts of a repo or a workspace, and an order other than alphabetical.
