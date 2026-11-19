# Load the selected Codex model before the first turn

- **Date:** 2026-11-19
- **Status:** accepted

## Context

A chat selected GPT-6.2 Sol before its own app-server had loaded the remote model catalog. Codex used fallback metadata for the first turn, with a smaller context window and direct tool exposure, and compacted repeatedly. Chauffeur's model picker reads the catalog in a separate process.

## Decision

Decided with Justin on 2026-11-19. Load the catalog in the chat's app-server while it starts. The first turn for an explicitly selected hosted model waits only while the model is still being confirmed, with a short deadline. An unconfirmed model leaves the message in the transcript with instructions to resend it or select another model.

## Consequences

The sidecar overlaps the catalog read with thread startup and gives the first send up to two seconds to finish the check. Chats using Codex's default selection or an Ollama model skip the check. Later turns proceed normally. This handles the startup race in Chauffeur. Codex owns its compaction loop and can address repeated ineffective compactions separately.
