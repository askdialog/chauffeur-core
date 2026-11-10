# A workspace share holds all its chats or the ones its owner picks, and an import's prompt continues a single chat

- **Date:** 2026-11-08
- **Status:** accepted

## Context

[0038](0038-workspace-shares.md) shared every chat of a workspace and left "sharing one chat alone" out of scope. Since then, a selection made when sharing (asked by Louis, 2026-10-29, in `SPEC.md` without an ADR) fixed the shared chats for the life of the link: every chat created later stayed private, and changing the selection meant stopping the share and sending a new link. Louis then imported a shared workspace with a first prompt and got two chats: the imported conversation, idle, and a new chat that started from his prompt without that conversation. He would rather have shared a single chat than all of them. Justin asked to let the owner choose.

## Decision

Asked by Justin 2026-11-08, from Louis's feedback, decided with Justin the same day:

- By default, a workspace share holds all its chats, those created later included, as 0038 had it.
- The owner can instead pick the chats to share, in the Share workspace dialog and with `chauffeur share-workspace --chats`. Only those reach the share service and the guests' page. A chat created later stays out of such a selection; only "All chats" follows new ones.
- The selection can change while the share is live, on the same link: a chat taken out leaves the service and the guests' page at once, with its suggestions and comments; a chat added is pushed with its history. Going from all chats to a selection, and back, works the same way.
- Importing a shared workspace with a first prompt ("Import into Chauffeur", "Import a shared workspace…" in New workspace, `chauffeur new --workspace-share <link> --prompt …`): when the share holds a single chat, the prompt continues that imported chat (its next message, the agent getting the imported conversation through the transcript handoff) instead of opening a second chat. A share of several chats still opens a new chat for the prompt.

This lifts "sharing one chat alone" from the out-of-scope list of [0038](0038-workspace-shares.md).

## Consequences

- `workspace.share_start` and `workspace.share_update` take `chat_ids` (the chats picked) or `all_chats`; without either, a new share holds all chats and an update keeps what the share had. `WorkspaceShareState.chat_ids` is absent when the share holds all chats. A share created before this change with a selection keeps it; one created before the selection existed keeps all its chats.
- The daemon owns the selection (`workspace_shares.chat_ids`, empty for all chats). On a change it sends the service the new chat list at once, the history of each chat added, and the suggestions of the shared chats only.
- The share service already dropped the events of a chat its daemon stops listing; it now drops that chat's suggestions and guests' comments too, and tells the guests. A chat closed in the app leaves the share the same way (only open chats are shared): reopened, it comes back with its history, but the guests' comments on it are gone. The owner's daemon keeps its own copy of those comments: the service's whole list now replaces only the comments of the chats still shared. An older daemon gains nothing and loses nothing: it sends the same lists.
- In New workspace, the preview of a share of one chat says that the prompt continues it. The continued chat takes the model, effort and permission mode chosen in the dialog, and the imported chat's title.
- Not done: picking which chat of several an import's prompt continues; sharing a selection of the notes.
