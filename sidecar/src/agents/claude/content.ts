// A user message for Claude: its images as image blocks (first, as Anthropic recommends), then
// the text, which names each attachment's file.

import { readFileSync } from "node:fs";
import type { SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import type { Attachment } from "../../protocol.ts";
import { attachmentNote, isImage } from "../attachments.ts";

/** The API takes images up to 5 MB once base64-encoded. */
export const MAX_INLINE_BYTES = 3_750_000;

type Content = Extract<SDKUserMessage["message"], { role: "user" }>["content"];
type MediaType = "image/png" | "image/jpeg" | "image/gif" | "image/webp";

export function userContent(
  text: string,
  attachments: Attachment[],
  cwd: string,
  read: (path: string) => Buffer = (path) => readFileSync(path),
): Content {
  if (attachments.length === 0) return text;
  const blocks: Exclude<Content, string> = [];
  const notInlined = new Set<Attachment>();
  for (const attachment of attachments) {
    let bytes: Buffer | undefined;
    if (!isImage(attachment)) continue;
    if (attachment.size <= MAX_INLINE_BYTES) {
      try {
        bytes = read(attachment.path);
      } catch {
        bytes = undefined;
      }
    }
    if (!bytes || bytes.length > MAX_INLINE_BYTES) {
      notInlined.add(attachment);
      continue;
    }
    blocks.push({
      type: "image",
      source: { type: "base64", media_type: attachment.media_type as MediaType, data: bytes.toString("base64") },
    });
  }
  const note = attachmentNote(attachments, cwd, (a) => !notInlined.has(a));
  blocks.push({ type: "text", text: `${text}${note}`.trimStart() });
  return blocks;
}
