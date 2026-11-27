# A design share's link shows a preview of the design to anyone who has it

- **Date:** 2026-11-27
- **Status:** accepted

## Context

A design share's link pasted in Slack (or any chat app) shows nothing but the bare URL: you have to open it to know what it is, and every link looks like every other. Bastien asked for an OpenGraph preview: the title, a description, and a view of the first screens. Until now, someone who was not signed in learned only the owner's name from a link; a chat app's crawler is never signed in, so a preview means showing more to anyone who has the link.

## Decision

Asked by Bastien, decided with Justin 2026-11-27:

- The page of a design share's link (`/s/<id>`) carries OpenGraph and Twitter card tags: the design's title, a description (its pages when there are several, its number of screens, "shared by <owner> on Chauffeur"), and an image of its first screens.
- Anyone who has the link sees them, signed in or not, and so does the chat app it is pasted in (its crawler, its cache, the people of the channel). The design itself (its files, its screens live, its comments) stays for the share's reviewers.
- The image is 1200 × 630: the first screens of the canvas, in its order, side by side at the image's height (up to three, fewer when they would be shrunk below half). The owner's daemon renders it with the headless Chrome of the design's PNG export and sends it to the share service; it is made when the link connects, then again after the design changes, at most every five minutes. Without Chrome the link shows its title and description alone.
- Designs only: the links of a shared markdown file or document and of a shared workspace show no preview. A stopped share's link shows none, and its image is removed with its files.

## Consequences

- The share service serves `/s/<id>/preview.png` without a session, and takes it from the owner's daemon on `PUT /v1/shares/<id>/preview` (a PNG of at most 5 MB). It is kept as a dotfile of the share's folder, never listed nor served as a design file.
- A design's title and first screens may be cached by a chat app after the share stops; the owner chooses where they paste the link.
- An owner cannot turn the preview off for one share; that would be a new decision.
