# Brands give decks and documents one look, shared by link

- **Date:** 2026-11-14
- **Status:** accepted

## Context

Decks and documents made by an agent take whatever look it improvises, and long decks drift. Claude Slides applies a design system set per user or organization. Chauffeur has repos' `DESIGN.md` for product screens, nothing for a company's brand. Justin asked for a brand at Chauffeur's level for slides and documents, shared among employees of one company (2026-11-14).

## Decision

Decided with Justin 2026-11-14:

- **A brand** is a folder `~/.chauffeur/brands/<id>/`: `BRAND.md` (tone, rules, do and don't, for the agent), `brand.css` (colours, fonts, sizes and spacing as CSS variables), `assets/` (logos for light and dark backgrounds, font files, brand images), `layouts/` (HTML models: title, section, content, quote, key figure, chart and closing slides; a document's cover, headings and footer) and a specimen page showing them.
- **Making one**: Settings › Brands › New brand takes a name and opens the brand editor, a full-screen view of its own: the brand chat beside its canvas, built like a design chat. A brand chat belongs to no workspace of the user: the daemon keeps every brand's chat in a workspace of its own (`~/.chauffeur/brands/.chats`), which no list, sidebar, search or review shows (Justin, 2026-11-16: a brand chat opened in whatever workspace was shown made no sense). Edit on a brand's row opens its chat again; a brand whose chat is gone, or lives in a user's workspace (made before the editor), gets a new one there; the user gives it what they have (a deck as PPTX or PDF, brand guidelines, a website address, a logo). Its agent writes only into the brand's folder, extracts colours, fonts and logos, writes the brand and shows the specimen and layouts on the canvas beside the chat; the user corrects it by chatting. Claude by default.
- **Using one**: several brands, one default, chosen in Settings › Brands. The composer has no brand picker: most chats never make a deck, and a picker in every composer clutters them (Justin, 2026-11-16). Every chat's agent is told the brands and may read their folders. With one brand, it builds with the default; with several, it asks in one short question, before building a deck or a document whose brand the user did not name, whether to use the default, another brand or none, and keeps that choice for the chat. It builds from the brand's layouts. A deck or document records its brand and copies its CSS, assets and fonts, so that it stays self-contained and a later change of the brand never alters it silently; its reader offers "The brand changed — apply".
- **Sharing a brand by link**, a later stage: a brand's Share opens the existing share dialog (named people, or "Anyone at <domain>"), which gives a link on the share service, hosted on Dialog's GCP for every user of Dialog's distribution. A colleague opening the link clicks "Add to Chauffeur": the brand joins their brands and can become their default. The link always serves the brand's latest state, and Chauffeur follows it, so colleagues get the author's changes without doing anything. Only the author changes it; others may duplicate it into a brand of their own. Stop sharing removes it from the service; colleagues keep their last copy. No catalogue, editors or publishing step.
- **Without the share service**: export and import of a `.chauffeur-brand` file, without updates.

## Consequences

- The brands' chats workspace is the daemon's: calls that would change it as a user's workspace (archive, rename, add a repo, share, transfer, a new chat) are refused as for a workspace that does not exist. A brand chat that ends a turn or asks for a permission notifies like any chat, under the name “Brands”.
- The daemon owns the brands' folders as the notes; the brand chat gets the same guards and sandbox as a design chat, pointed at the brand's folder.
- Agents read the chosen brand through an allow rule, as the notes; Codex chats are told its path.
- Publishing a brand holding font files warns that a commercial font's licence may forbid sharing, and offers to publish the font's name and fallback only.
- A shared brand is a new kind of share on the existing service, kept with the other shares (its SQLite and files on Dialog's GCP), with the same access as any share: named invitees, or the owner's team domain ([share-with-any-email](share-with-any-email.md)). No company administrator is introduced. A build of Core without a configured share service ([core-open-source-without-dialog-defaults](core-open-source-without-dialog-defaults.md)) has export and import only, or hosts the service itself.
- Dialog stores other companies' brand files (logos, fonts): the share service's terms must cover them.
- Left for later: setting a brand by hand (colour pickers, uploading a logo), editors other than the author, an official brand pinned for a company, PowerPoint templates (`.potx`) as a brand's source of truth.
