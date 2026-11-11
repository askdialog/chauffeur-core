# Architecture decision records

One file per decision settled with Justin: what was decided, when, why, and what follows from it. [`SPEC.md`](../SPEC.md) stays the reference for how Chauffeur behaves; an ADR records the decision behind a behaviour.

The first 38 ADRs were extracted from `SPEC.md` on 2026-10-29: each groups the dated decisions of one subject ("decided with Justin", "approved by Justin", "asked by Justin" and the like), and their Decision section copies the SPEC's text word for word. Ideas still proposed or awaiting Justin (the composer's live code preview, parts of the Office) have no ADR.

## Writing one

A new decision settled with Justin gets an ADR, in the same PR as the change it allows:

1. Name it by a short slug, without a number: `docs/adr/<slug>.md`. ADRs 0001 to 0053 keep their numbers; new ones have none, so parallel pull requests never compete for the same one ([adrs-named-by-slug](adrs-named-by-slug.md)).
2. Fill in the sections below. The Decision is the decision as agreed, with its date and who asked for it ("asked by a user, decided with Justin 2026-11-02"); do not soften or widen it.
3. If the behaviour changes, update `SPEC.md` too, citing the ADR (`see docs/adr/<slug>.md`), so that the SPEC keeps describing Chauffeur as it is.
4. Run `.github/scripts/adr_index.py` and commit the index it writes below. Do not edit the index by hand: CI fails when it is stale.

```markdown
# <The decision, as a title>

- **Date:** YYYY-MM-DD
- **Status:** accepted

## Context

What made a decision necessary: the problem, the constraint, who asked. Two to five lines.

## Decision

What was decided, as agreed.

## Consequences

What follows: what the code must and must not do, what it costs, what is left for later.
```

A decision that replaces an earlier one is a new ADR. The earlier one keeps its text, and only its status changes: `superseded by [<slug>](<slug>.md)`.

## Index

<!-- index -->

| Decision | Date | Status |
|---|---|---|
| [0001. Chauffeur V1: one workspace across several repos](0001-v1-scope.md) | 2026-10-13 | accepted |
| [0002. Explorations: read-only workspaces at the base branch](0002-explorations.md) | 2026-10-14 | accepted |
| [0003. Questions, permission requests and Always allow](0003-questions-and-permissions.md) | 2026-10-14 | accepted |
| [0004. Pull requests, merging and archiving](0004-pull-requests-merging-and-archiving.md) | 2026-10-14 | accepted |
| [0005. Creating and naming a workspace](0005-creating-and-naming-a-workspace.md) | 2026-10-15 | accepted |
| [0006. What a chat's transcript shows](0006-chat-transcript.md) | 2026-10-15 | accepted |
| [0007. Resuming cut turns and closing idle sessions](0007-resuming-chats-and-idle-sessions.md) | 2026-10-15 | accepted |
| [0008. Compaction through Jev](0008-jev-compaction.md) | 2026-10-15 | accepted |
| [0009. Settings window, appearance and defaults](0009-settings.md) | 2026-10-15 | accepted |
| [0010. Plan usage, lost logins and broken MCP servers](0010-plan-usage-logins-and-mcp-servers.md) | 2026-10-15 | accepted |
| [0011. Sidebar order, sections, child folding and notification sounds](0011-sidebar.md) | 2026-10-16 | accepted |
| [0012. Composer: steering, permission mode, drafts and prompt suggestions](0012-composer.md) | 2026-10-16 | accepted |
| [0013. Background tasks and subagents in a chat](0013-background-tasks-and-subagents.md) | 2026-10-16 | accepted |
| [0014. Linear tickets attached after creation](0014-linear-tickets-after-creation.md) | 2026-10-16 | accepted |
| [0015. Attachments and the images agents look at](0015-attachments-and-images.md) | 2026-10-16 | accepted |
| [0016. Keeping the machine awake while agents work](0016-keep-awake.md) | 2026-10-17 | accepted |
| [0017. Chat tabs and forks](0017-chat-tabs-and-forks.md) | 2026-10-18 | accepted |
| [0018. Design chats and their canvas](0018-design-chats.md) | 2026-10-18 | accepted |
| [0019. Workspace view, terminal tabs and run scripts](0019-workspace-view-terminals-and-run-scripts.md) | 2026-10-18 | accepted |
| [0020. Codex in a multi-repo workspace](0020-codex-in-a-multi-repo-workspace.md) | 2026-10-18 | accepted |
| [0021. The daemon's commands refused from inside a workspace](0021-daemon-commands-from-inside.md) | 2026-10-18 | accepted |
| [0022. Keyboard shortcuts, custom bindings and workspace history](0022-shortcuts-and-navigation.md) | 2026-10-18 | accepted |
| [0023. Mobile companion, relay and pairing](0023-mobile-companion.md) | 2026-10-18 | accepted |
| [0024. Repos the agent proposes to add](0024-repos-the-agent-proposes.md) | 2026-10-19 | accepted |
| [0025. Subagent chats through `delegate`](0025-subagent-chats.md) | 2026-10-19 | accepted |
| [0026. Files tab: media, Finder and HTML preview](0026-files-tab.md) | 2026-10-19 | accepted |
| [0027. Cloud workspace handoff scope](0027-cloud-workspace-handoff-scope.md) | 2026-10-19 | accepted |
| [0028. Mobile store preparation](0028-mobile-store-preparation.md) | 2026-10-20 | accepted |
| [0029. Comments on design screens and live design shares](0029-design-comments-and-shares.md) | 2026-10-21 | sign-in and team superseded by [share-with-any-email](share-with-any-email.md); who sees the reviewers by [mentions-in-share-comments](mentions-in-share-comments.md) |
| [0030. Transfers of a workspace to another machine and back](0030-transfers.md) | 2026-10-23 | accepted |
| [0031. Piloting a moved workspace from the Mac](0031-piloting-a-moved-workspace.md) | 2026-10-26 | accepted |
| [0032. Development data on a transfer](0032-development-data-on-a-transfer.md) | 2026-10-26 | accepted |
| [0033. GCP workers: creation, on-demand lifecycle, sign-in and remote editor](0033-gcp-workers.md) | 2026-10-26 | accepted |
| [0034. Shell commands in a chat (`!`)](0034-shell-commands-in-a-chat.md) | 2026-10-26 | accepted |
| [0035. Backups of a workspace running on a GCP worker](0035-worker-backups.md) | 2026-10-27 | accepted |
| [0036. Release automation](0036-release-automation.md) | 2026-10-28 | accepted |
| [0037. Local models through Ollama](0037-local-models.md) | 2026-10-29 | accepted |
| [0038. Sharing a workspace live, with suggestions](0038-workspace-shares.md) | 2026-10-29 | sign-in and team superseded by [share-with-any-email](share-with-any-email.md), sharing chosen chats allowed by [share-chosen-chats](share-chosen-chats.md) |
| [0039. Changing a chat's provider with confirmation](0039-changing-a-chats-provider.md) | 2026-10-29 | accepted |
| [0040. Render code review comments in chat](0040-code-comments.md) | 2026-10-29 | accepted |
| [0041. Archived search uses the archive date](0041-archive-search-order.md) | 2026-10-29 | accepted |
| [0042. PostgreSQL inside Chauffeur](0042-postgresql-client.md) | 2026-10-29 | accepted |
| [0043. Touch ID before non-SELECT SQL](0043-database-touch-id.md) | 2026-10-29 | accepted |
| [0044. Fall back to Codex for workspace titles](0044-title-provider-fallback.md) | 2026-10-29 | accepted |
| [0047. A chat started by `delegate` is a delegated chat](0047-delegated-chats.md) | 2026-10-29 | accepted |
| [0049. Transfer confirmation closes immediately](0049-background-transfer-confirmation.md) | 2026-10-29 | accepted |
| [0050. Release summaries through a Claude subscription](0050-release-summary-subscription.md) | 2026-10-29 | accepted |
| [0052. Prefill a new local workspace from a Slack message](0052-slack-new-workspace.md) | 2026-10-29 | superseded by [slack-direct-desktop](slack-direct-desktop.md) |
| [0045. Identify daily review workspaces with a badge](0045-daily-review-badge.md) | 2026-10-30 | accepted |
| [0046. Pull requests are checked on Linux, macOS is checked at release](0046-linux-checks-per-pull-request.md) | 2026-10-30 | accepted |
| [0048. One agent signed in on a worker is enough for a transfer](0048-one-agent-signed-in-on-a-worker.md) | 2026-10-30 | accepted |
| [0051. GitHub without Homebrew: Chauffeur's own `gh`, signed in in the browser](0051-github-without-homebrew.md) | 2026-10-30 | accepted |
| [0053. Create a design from Command N](0053-create-design-from-command-n.md) | 2026-10-30 | accepted |
| [ADRs are named by their slug, without a number](adrs-named-by-slug.md) | 2026-10-30 | accepted |
| [The credential vault and the relay-hosted controller are removed](credential-vault-removed.md) | 2026-10-31 | accepted |
| [A worker gets the Mac's GitHub and AWS SSO access on each send](worker-github-and-aws-access.md) | 2026-10-31 | accepted |
| [Archiving a workspace that runs on another machine brings it back first](archive-a-moved-workspace.md) | 2026-11-02 | accepted |
| [Design chats run on Codex too](codex-design-chats.md) | 2026-11-02 | accepted |
| [Open Slack context directly in the desktop](slack-direct-desktop.md) | 2026-11-02 | accepted |
| [A worker gets the Mac's Claude Code configuration on each send](worker-claude-code-configuration.md) | 2026-11-02 | accepted |
| [A worker installs the toolchains each repo declares](worker-toolchains.md) | 2026-11-02 | accepted |
| [A transfer updates a worker that runs another Chauffeur version](worker-updated-by-a-transfer.md) | 2026-11-02 | accepted |
| [Error toasts are reported to Sentry, as the user chooses](error-reports-to-sentry.md) | 2026-11-03 | accepted |
| [Chauffeur Core with official bundled extensions](official-bundled-extensions.md) | 2026-11-03 | accepted |
| [A guest deletes their suggestion and comments on a passage of an agent's message](share-guest-deletions-and-comments.md) | 2026-11-03 | accepted |
| [The share service's image is built off the relay's VM](share-image-built-off-the-vm.md) | 2026-11-03 | accepted |
| [A transfer carries the workspace folder's files outside its repos](transfer-carries-files-outside-the-repos.md) | 2026-11-03 | accepted |
| [The daemon follows git for a workspace's worktrees](worktrees-follow-git.md) | 2026-11-03 | accepted |
| [⌘Enter writes a design comment and sends it to the agent](comment-and-send-with-cmd-enter.md) | 2026-11-04 | accepted |
| [Official extensions download and update independently](independent-official-extension-updates.md) | 2026-11-04 | accepted |
| [Repos of every GitHub owner the account reaches, picked in a grouped picker](repos-of-every-github-owner.md) | 2026-11-04 | accepted |
| [Core and official extensions have separate source repositories](separate-extension-repositories.md) | 2026-11-04 | accepted |
| [Colleagues named on a share get an e-mail, sent through Amazon SES](share-invitation-emails.md) | 2026-11-04 | sender superseded by [share-invitation-sender](share-invitation-sender.md), recipients by [share-with-any-email](share-with-any-email.md) |
| [Core is published as open source, with no default pointing at Dialog's services](core-open-source-without-dialog-defaults.md) | 2026-11-05 | accepted; license resolved by [core-mit-license](core-mit-license.md) |
| [The macOS app can be signed with a Developer ID and notarized](developer-id-signing-and-notarization.md) | 2026-11-05 | accepted |
| [The new-workspace dialog puts the prompt first](new-workspace-prompt-first.md) | 2026-11-05 | accepted |
| [The iOS and Android companion has its own source repository](separate-mobile-repository.md) | 2026-11-05 | accepted |
| [Invitations come from the operator's sender, of a subdomain verified alone](share-invitation-sender.md) | 2026-11-05 | accepted |
| [Import from Conductor in one command](conductor-import.md) | 2026-11-06 | accepted |
| [File daily reviews in a folded section of their own](daily-reviews-in-their-own-section.md) | 2026-11-06 | accepted |
| [Interface mockup requests use Chauffeur design chats](design-requests-use-design-chats.md) | 2026-11-06 | accepted |
| [Create a design without repos](designs-without-repos.md) | 2026-11-06 | accepted |
| [Import public Claude Design canvases](import-public-claude-designs.md) | 2026-11-06 | accepted |
| [Features relying on a distribution's hosted services are for its members only](members-only-features.md) | 2026-11-06 | accepted |
| [Rename a workspace from recent messages](rename-from-recent-messages.md) | 2026-11-06 | accepted |
| [Design and workspace shares on hosts of their own, one sign-in for both](share-hosts-per-kind.md) | 2026-11-06 | accepted |
| [Shares open to any e-mail address, with Google or a sign-in link](share-with-any-email.md) | 2026-11-06 | accepted |
| [Use Apple's system cryptography in the iOS companion](ios-system-cryptography.md) | 2026-11-07 | accepted |
| [Search every workspace on the machine, by keywords and by meaning](local-hybrid-search.md) | 2026-11-08 | accepted |
| [Mentioning people in the comments of a shared design or workspace](mentions-in-share-comments.md) | 2026-11-08 | accepted |
| [Mobile appearance follows the system or an explicit preference](mobile-appearance.md) | 2026-11-08 | accepted |
| [Focused mobile navigation and separate preferences](mobile-focused-navigation.md) | 2026-11-08 | accepted |
| [Review shared designs inside the mobile companion](mobile-hosted-review.md) | 2026-11-08 | accepted |
| [Mobile model selection and the Mac's workspace organization](mobile-models-and-workspace-hierarchy.md) | 2026-11-08 | accepted |
| [Open private Canvas screens on the phone](mobile-private-canvas.md) | 2026-11-08 | accepted |
| [Mobile dictation through Voxtral and the GCP relay](mobile-voxtral-dictation.md) | 2026-11-08 | accepted |
| [Routines: a prompt Chauffeur runs by itself at set times, born from a chat](routines.md) | 2026-11-08 | accepted |
| [A workspace share holds all its chats or the ones its owner picks, and an import's prompt continues a single chat](share-chosen-chats.md) | 2026-11-08 | accepted |
| [A markdown file is commented on, shared alone, and its comments are sent to the chat that wrote it](comment-and-share-markdown-files.md) | 2026-11-09 | accepted |
| [Only Chauffeur Core is released under MIT](core-mit-license.md) | 2026-11-09 | accepted |
| [A delegated chat runs on its parent's agent, a review on the other](delegate-agent-and-model.md) | 2026-11-09 | accepted |
| [A delegated chat outlives its parent's turn, and its answer comes as a message](delegated-chats-outlive-the-parents-turn.md) | 2026-11-09 | accepted |
| [Find routines: proposing routines from the user's own recent requests](find-routines.md) | 2026-11-09 | accepted |
| [One design comment thread is sent to the agent alone](send-one-comment-thread.md) | 2026-11-09 | accepted |
| [HTML and PDF deliverables stay in their conversation](chat-document-deliverables.md) | 2026-11-10 | accepted |
| [Core CI checks the changed surface and packages Linux on a separate schedule](core-ci-by-changed-surface.md) | 2026-11-10 | accepted |
| [Core starts with a new GitHub repository and a single initial commit](core-fresh-public-history.md) | 2026-11-10 | accepted |
| [Independent extension releases share the Core download repository](independent-extension-releases-on-core.md) | 2026-11-10 | accepted |
| [Manage outgoing shares and keep archived designs shared](manage-outgoing-shares.md) | 2026-11-10 | accepted |
| [Separate Core and Dialog release artifacts](separate-core-and-dialog-releases.md) | 2026-11-11 | accepted |
