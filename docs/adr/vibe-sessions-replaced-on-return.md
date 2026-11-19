# A Mistral Vibe conversation coming back replaces the copy it left

- **Date:** 2026-11-18
- **Status:** accepted

## Context

A transfer restores conversations into the destination's native homes as new files, never replacing one: Claude's and Codex's transcripts only grow, so a conversation coming back extends the copy it left. A Vibe session is a folder Vibe rewrites as the conversation goes (`CURRENT` points to the latest generation, `meta.json` changes, old generations are deleted). The copy the Mac kept cannot take the version coming back from a worker by adding files. Justin chose how to bring it back.

## Decision

Decided with Justin 2026-11-18:

- **A Vibe conversation is carried whole.** A transfer carries its session folder (`logs/session/unified/<id>` of the Vibe home) and the plugin environment it pins (manifests, packages and blobs of `logs/session/plugins`, named by their digest).
- **Coming back, it replaces the copy it left.** When the destination already has that session's folder, the restoration sets it aside whole and puts the carried one in its place. The folder set aside is deleted once the restoration succeeded, and put back when the return fails.
- **Nothing else is replaced.** The exception is that session's folder only. A plugin file already there must hold the same bytes, or the restoration refuses; Claude's and Codex's conversations keep the rule of new files only.

## Consequences

- The sidecar sets a replaced folder aside in `logs/session/chauffeur-replaced/` of the Vibe home, and names it in the restoration's result (`replaced`). The daemon records it with what the restoration wrote (`NativeFiles::replaced`), deletes it after a successful restoration, and puts it back after removing what a failed return wrote. A record that points elsewhere is left alone.
- The copy a machine deletes once the workspace left it for good takes its Vibe session folders whole: Vibe wrote new files there beside the restored ones.
- Vibe hard-links a plugin package's files to their blobs. The capture reads those files whatever their links, but carries one only when its content is the digest that names it.
- The destination's Vibe home is `VIBE_HOME`, else `~/.vibe`. A destination without Vibe refuses a workspace with a Vibe chat before anything stops. Workers install Vibe's standalone `vibe-acp` at the version the sidecar was tried with, checked against its SHA-256; it keeps its key in `~/.vibe/.env` there, with no keyring.
