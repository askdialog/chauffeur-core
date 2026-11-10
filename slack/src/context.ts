export type Message = {
  text: string;
  ts: string;
  user?: string;
  username?: string;
  thread_ts?: string;
  reply_count?: number;
  files?: { title?: string; name?: string; permalink?: string }[];
  attachments?: string[];
};

export type Shortcut = {
  type: "message_action";
  callback_id: "chauffeur_open";
  trigger_id: string;
  team: { id: string; domain: string };
  channel: { id: string; name?: string };
  user: { id: string };
  message: Message;
};

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function blockText(value: unknown, depth = 0): string {
  if (depth > 8) return "";
  if (Array.isArray(value)) return value.map((item) => blockText(item, depth + 1)).filter(Boolean).join("\n");
  if (!object(value)) return "";
  if (typeof value.text === "string") return value.text;
  if (object(value.text)) return blockText(value.text, depth + 1);
  if (value.type === "user" && typeof value.user_id === "string") return `<@${value.user_id}>`;
  if (value.type === "link" && typeof value.url === "string") return value.url;
  if (Array.isArray(value.elements)) return value.elements.map((item) => blockText(item, depth + 1)).join(value.type === "rich_text_section" ? "" : "\n");
  if (Array.isArray(value.fields)) return blockText(value.fields, depth + 1);
  return "";
}

export function parseMessage(value: unknown): Message | null {
  if (!object(value) || typeof value.ts !== "string" || !/^\d+\.\d+$/.test(value.ts)) return null;
  if (value.text !== undefined && typeof value.text !== "string") return null;
  const message: Message = { text: typeof value.text === "string" ? value.text : "", ts: value.ts };
  if (!message.text) message.text = blockText(value.blocks);
  if (Array.isArray(value.attachments)) {
    message.attachments = value.attachments.filter(object).map((attachment) => {
      const text = [attachment.pretext, attachment.title, attachment.text].filter((part) => typeof part === "string");
      if (Array.isArray(attachment.fields)) for (const field of attachment.fields.filter(object)) {
        text.push([field.title, field.value].filter((part) => typeof part === "string").join(": "));
      }
      const blocks = blockText(attachment.blocks);
      if (blocks) text.push(blocks);
      if (!text.length && typeof attachment.fallback === "string") text.push(attachment.fallback);
      return text.join("\n");
    }).filter(Boolean);
  }
  if (typeof value.user === "string") message.user = value.user;
  if (typeof value.username === "string") message.username = value.username;
  if (typeof value.thread_ts === "string" && /^\d+\.\d+$/.test(value.thread_ts)) message.thread_ts = value.thread_ts;
  if (typeof value.reply_count === "number") message.reply_count = value.reply_count;
  if (Array.isArray(value.files)) {
    message.files = value.files.filter(object).map((file) => ({
      title: typeof file.title === "string" ? file.title : undefined,
      name: typeof file.name === "string" ? file.name : undefined,
      permalink: typeof file.permalink === "string" && /^https:\/\/[^/]+\.slack\.com\//.test(file.permalink) ? file.permalink : undefined,
    }));
  }
  return message;
}

export function parseShortcut(value: unknown): Shortcut | null {
  if (!object(value) || value.type !== "message_action" || value.callback_id !== "chauffeur_open"
    || typeof value.trigger_id !== "string" || !object(value.team) || !object(value.channel) || !object(value.user)) return null;
  const { team, channel, user } = value;
  if (typeof team.id !== "string" || typeof team.domain !== "string" || !/^[a-zA-Z0-9-]+$/.test(team.domain)
    || typeof channel.id !== "string" || !/^[A-Z0-9]+$/.test(channel.id) || typeof user.id !== "string") return null;
  const message = parseMessage(value.message);
  if (!message) return null;
  return {
    type: "message_action", callback_id: "chauffeur_open", trigger_id: value.trigger_id,
    team: { id: team.id, domain: team.domain },
    channel: { id: channel.id, name: typeof channel.name === "string" ? channel.name : undefined },
    user: { id: user.id }, message,
  };
}

export function formatContext(shortcut: Shortcut, messages: Message[], thread: boolean): string {
  const root = thread ? shortcut.message.thread_ts ?? shortcut.message.ts : shortcut.message.ts;
  const source = `https://${shortcut.team.domain}.slack.com/archives/${shortcut.channel.id}/p${root.replace(".", "")}`;
  const heading = `${thread ? "Fil" : "Message"} Slack — #${shortcut.channel.name ?? shortcut.channel.id}\n${source}`;
  return [heading, ...messages.map((message) => {
    const author = message.username ?? message.user ?? "Slack";
    const at = new Date(Number(message.ts) * 1000);
    const date = Number.isNaN(at.getTime()) ? message.ts : at.toISOString();
    // Slack escapes these three characters in mrkdwn. Keep its links and mentions as source text.
    const text = [message.text, ...(message.attachments ?? [])].filter(Boolean).join("\n\n")
      .replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");
    const files = (message.files ?? []).map((file) => `${file.title ?? file.name ?? "Pièce jointe"}${file.permalink ? `: ${file.permalink}` : " (à consulter dans Slack)"}`);
    return `${author} — ${date}\n${[text, ...files].filter(Boolean).join("\n")}`;
  })].join("\n\n");
}
