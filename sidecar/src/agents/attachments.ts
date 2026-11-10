// Files the user sent with a message, as the daemon saved them in the workspace's notes folder:
// images go with the message, any other file (a zip, an HTML page) is named for the agent to open.

import type { Attachment } from "../protocol.ts";

const IMAGE_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);

/** An image the agent can be shown; anything else is a file it reads with its tools. */
export function isImage(attachment: Attachment): boolean {
  return IMAGE_TYPES.has(attachment.media_type);
}

/**
 * A line per attachment naming its file, so the agent can refer to it (put it in a PR, compare it
 * with a later screenshot) without reading again what it already sees, and open the files it was
 * not shown; relative to the agent's folder when it is inside. `shown` says whether an image itself
 * went with the message.
 */
export function attachmentNote(attachments: Attachment[], cwd: string | null, shown: (a: Attachment) => boolean = () => true): string {
  if (attachments.length === 0) return "";
  const lines = attachments.map((attachment) => {
    const path = cwd && attachment.path.startsWith(`${cwd}/`) ? attachment.path.slice(cwd.length + 1) : attachment.path;
    if (!isImage(attachment)) return `[The user attached the file ${JSON.stringify(attachment.name)} (${attachment.media_type}); it is saved at ${path}]`;
    return shown(attachment)
      ? `[The attached image is shown above; it is also saved at ${path}]`
      : `[An image is attached at ${path}: too large to show here, read it with the Read tool]`;
  });
  return `\n\n${lines.join("\n")}`;
}
