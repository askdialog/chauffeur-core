# Search every workspace on the machine, by keywords and by meaning

- **Date:** 2026-11-08
- **Status:** accepted

## Context

⌘K matched only a workspace's slug, title, tickets, branch and repos; conversations were searched in archived workspaces alone (`workspace.search_archived`, [0041](0041-archive-search-order.md)), by their exact words. Past work is looked for by what was said ("the fix for the doubled webhooks"), in French or English, often without the words used then. Transcripts are the user's own, and Core is open source.

## Decision

Decided with Justin 2026-11-08:

- Everything stays local: no hosted vector database (Turbopuffer was considered and rejected: it would send every transcript to a third party and add a vendor to an open-source Core), no hosted embedding API.
- Keywords: SQLite FTS5 (BM25 ranking, phrases, prefixes, highlighted excerpts; the `unicode61` tokenizer with diacritics removed, the corpus mixing French and English).
- Meaning: sqlite-vec (vectors stored next to filterable metadata, nearest-neighbour search), with embeddings computed by a light local model that the Ollama Chauffeur already installs, starts and stops serves ([0037](0037-local-models.md)), through its `/api/embed`. The model is multilingual (French, English, code) and set in the settings. embeddinggemma (300M) and qwen3-embedding:0.6b were compared on a synthetic sample (40 messages in French, English and code, 27 queries sharing no word with their target): the same quality (recall@5 1.00 and 0.96, MRR 0.981 and 0.966), but embeddinggemma indexes 1.8× faster (about 32 pieces of 1,200 characters a second on an M5 Pro, against 18), takes 686 MB loaded against 988 MB, and separates relevant passages (similarity 0.32 and up) from unrelated ones (0.25 at most) where qwen3's ranges overlap (0.36 and up against 0.44 at most). embeddinggemma is the default.
- Both result lists are merged by Reciprocal Rank Fusion. Without Ollama, without the model, or before the history is indexed, keywords alone still work, and the app says that search by meaning is off, or how far the indexing is.

Proposed to Justin the same day, without objection:

- In ⌘K, one search over active and archived workspaces: their titles, the user's and the agents' messages (not tool outputs), the notes folder's markdown, the pull requests' titles, the tickets.
- Filters: repo, ticket, date, active or archived, with a pull request or with one merged, and where to look (titles, messages, notes).
- A result opens the matching message in its chat.

## Consequences

- The daemon owns the index, `search.db` in its state folder (0600): it reads the store attached read-only and never writes it, so the filters read workspaces as they are now, and deleting the file rebuilds everything. The full-text table the store kept by triggers on every event for archived search is dropped.
- The index follows the event log as events are logged, with the titles and the active workspaces' notes; archived workspaces' notes and deletions are read again every ten minutes. A background task gives the history its vectors, newest first, sixteen pieces at a time, resting as long as each batch took. On battery it never starts Ollama for that, and gives one batch at most to an Ollama already running; a search may start it, since the user asked. Requests keep the model loaded one minute (five after a search), so that the Ollama the daemon started still stops once unused, as before. The battery is read on macOS only (`pmset`): elsewhere the machine counts as on the charger.
- The embedding model is never downloaded on its own: ⌘K and Settings › Search offer it, with its size. Changing the model gives everything its vectors again. Embedding models are left out of the chat model picker.
- Pull requests are kept as the daemon reads them (a workspace's PR status); those of the workspaces before this index are looked for once on GitHub through `gh`, a few workspaces a pass, waiting half an hour after `gh` fails.
- `workspace.search_archived` stays for older clients, answered by the new index. With the archived scope alone (the sidebar's Archived (n)), results keep 0041's order: the newest archive first, then relevance.
- sqlite-vec scans every vector of the filtered workspaces: an index of its own is for when a history outgrows that. A `chauffeur search` command is left for later.
