# A chat's images are saved to Downloads by the desktop shell

- **Date:** 2026-11-13
- **Status:** accepted

## Context

A user asked to download the images of a chat from the lightbox and the thumbnails, and all the images of a message at once (asked by a user, 2026-11-12). The daemon's `/attachments` URL holds the app's token, so it must not be opened in the system browser; the web view cannot save a file; the app has no dialog or file-system plugin.

## Decision

The desktop shell saves them: the UI hands the daemon's attachment URLs and names to the `save_downloads` command, which fetches them and writes them to the Downloads folder under their original name (`name (2).ext` when taken, nothing overwritten). Several files go into one new folder, `Chauffeur attachments`, instead of a ZIP. The UI says where they went. The command fetches only the daemon's `/attachments/` route. Decided with Justin 2026-11-13.

## Consequences

No new dependency and no "Save as" dialog: the user finds the files in Downloads. Only images can be saved, since the daemon serves only images ([0015](0015-attachments-and-images.md)); other files stay reachable by their path. A plain browser is offered nothing: it ignores a `download` link across origins and the daemon allows no cross-origin fetch. A "Save as" dialog or a ZIP can come later if asked for.
