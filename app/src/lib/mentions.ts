// Mentions in the comments of a shared design or a shared workspace
// (docs/adr/mentions-in-share-comments.md), kept pure so that they are tested in mentions.test.ts:
// the `@query` being typed, the people it offers, inserting one, which mentions a text still holds,
// the text cut into its highlighted parts, and what is unread.

import type { CommentAuthor, Mention, Mentioned } from "@chauffeur/protocol";

/** A comment as far as its thread and mentions go: a design's or a workspace's. */
type Threaded = { id: string; reply_to?: string; mentions?: Mention[] };

/** The `@…` being typed just before the caret: where its `@` is, and what follows it. */
export type MentionQuery = { start: number; query: string };

const EMAIL = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;

/**
 * The word the caret ends, when it starts with `@` at the start of the text or after a space or
 * an opening bracket: `@bo|` → "bo", `@bob@x.co|` → "bob@x.co". None inside an e-mail
 * (`alice@x|`) or once a space follows.
 */
export function mentionQuery(text: string, caret: number): MentionQuery | null {
  let start = caret;
  while (start > 0 && !/\s/.test(text[start - 1])) start -= 1;
  // An opening bracket may come first: "(@bo".
  while (start < caret && "([{".includes(text[start])) start += 1;
  if (start >= caret || text[start] !== "@") return null;
  return { start, query: text.slice(start + 1, caret) };
}

/** A name for an address the service has none for yet: its first part, capitalized, as the service makes it. */
export function firstName(email: string): string {
  const local = email.split("@")[0] ?? email;
  const first = local.split(/[._-]/)[0] ?? local;
  return first.charAt(0).toUpperCase() + first.slice(1);
}

/**
 * What the list under the text box offers: someone with access, an address of the team domain, an
 * invitation, or (not to be picked) an address without access whose owner alone may invite it.
 */
export type MentionOption = { person: Mention; kind: "person" | "team" | "invite" | "no_access" };

/**
 * The people `query` matches (a name's word or the e-mail starting with it, anywhere else after),
 * never `me`; then, for a full address no one has: an address of the team domain, or, when the
 * user may invite, "Invite and mention", else, when the owner is named, who to ask.
 */
export function mentionOptions(
  people: readonly CommentAuthor[],
  query: string,
  { me, teamDomain, canInvite, owner }: { me?: string | null; teamDomain?: string | null; canInvite?: boolean; owner?: string | null },
): MentionOption[] {
  const q = query.trim().toLowerCase();
  const others = people.filter((p) => p.email !== me);
  const starts = (p: CommentAuthor) => p.email.startsWith(q) || p.name.toLowerCase().split(/\s+/).some((word) => word.startsWith(q));
  const contains = (p: CommentAuthor) => p.email.includes(q) || p.name.toLowerCase().includes(q);
  const matched = q ? [...others.filter(starts), ...others.filter((p) => !starts(p) && contains(p))] : others;
  const options: MentionOption[] = matched.map((p) => ({ person: { email: p.email, name: p.name }, kind: "person" }));
  if (EMAIL.test(q) && !people.some((p) => p.email === q) && q !== me) {
    const person = { email: q, name: firstName(q) };
    if (teamDomain && q.endsWith(`@${teamDomain.toLowerCase()}`)) options.push({ person, kind: "team" });
    else if (canInvite) options.push({ person, kind: "invite" });
    else if (owner) options.push({ person, kind: "no_access" });
  }
  return options;
}

/** The text with `@query` before the caret replaced by `@Name `, and where the caret goes. */
export function insertMention(text: string, at: MentionQuery, caret: number, person: Mention): { text: string; caret: number } {
  const inserted = `@${person.name} `;
  const after = text.slice(caret).replace(/^ /, "");
  return { text: text.slice(0, at.start) + inserted + after, caret: at.start + inserted.length };
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** `@Name` as a whole word: not `@Bobby` for Bob. */
function mentionPattern(name: string, flags = ""): RegExp {
  return new RegExp(`@${escapeRegExp(name)}(?![\\p{L}\\p{N}_])`, `u${flags}`);
}

/** The people picked whose `@Name` the text still holds, each once, in the order picked. */
export function mentionsIn(text: string, picked: readonly Mention[]): Mention[] {
  const kept: Mention[] = [];
  for (const person of picked) {
    if (kept.some((p) => p.email === person.email)) continue;
    if (mentionPattern(person.name).test(text)) kept.push(person);
  }
  return kept;
}

/** A comment's text in parts, its mentions apart so that they are highlighted. */
export type TextPart = { text: string; mention?: Mention };

export function textParts(text: string, mentions: readonly Mention[] | undefined): TextPart[] {
  if (!mentions || mentions.length === 0 || !text) return [{ text }];
  // The longest names first: "@Anne Marie" before "@Anne".
  const sorted = [...mentions].sort((a, b) => b.name.length - a.name.length);
  const parts: TextPart[] = [];
  let rest = text;
  while (rest) {
    let best: { index: number; mention: Mention; length: number } | null = null;
    for (const mention of sorted) {
      const found = mentionPattern(mention.name).exec(rest);
      if (found && (best == null || found.index < best.index)) best = { index: found.index, mention, length: found[0].length };
    }
    if (!best) {
      parts.push({ text: rest });
      break;
    }
    if (best.index > 0) parts.push({ text: rest.slice(0, best.index) });
    parts.push({ text: rest.slice(best.index, best.index + best.length), mention: best.mention });
    rest = rest.slice(best.index + best.length);
  }
  return parts;
}

/** Whether a comment mentions `email`. */
export function mentions(comment: Pick<Threaded, "mentions">, email: string): boolean {
  return (comment.mentions ?? []).some((m) => m.email === email);
}

/** The thread a comment is in: its own id when it opens one. */
export function threadOf(comment: Threaded): string {
  return comment.reply_to ?? comment.id;
}

/** The threads holding unread mentions of the reader (`unread`: the comments' ids), in the comments' order. */
export function unreadThreads(comments: readonly Threaded[], unread: readonly string[]): string[] {
  const threads: string[] = [];
  for (const comment of comments) {
    if (!unread.includes(comment.id)) continue;
    const thread = threadOf(comment);
    if (!threads.includes(thread)) threads.push(thread);
  }
  return threads;
}

/** The unread ones of the user's mentions. */
export function unreadCount(all: readonly Mentioned[]): number {
  return all.filter((m) => !m.seen).length;
}

/** A comment cut short for a list or a notification. */
export function excerpt(text: string, max = 120): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

/** The thread a share page's address asks for (`?thread=<id>`), if any. */
export function threadParam(search: string): string | null {
  return new URLSearchParams(search).get("thread");
}

/** The chat and the comment a workspace share's address asks for (`?chat=<id>&comment=<id>`), if any. */
export function commentParam(search: string): { chat: string; comment: string | null } | null {
  const params = new URLSearchParams(search);
  const chat = params.get("chat");
  return chat ? { chat, comment: params.get("comment") } : null;
}

/**
 * The comments that mention `me` for the first time in `after`, by someone else: what the reader
 * is told of ("Bob mentioned you").
 */
export function newMentions<C extends Threaded & { author: CommentAuthor }>(before: readonly C[], after: readonly C[], me: string): C[] {
  const known = new Set(before.filter((c) => mentions(c, me)).map((c) => c.id));
  return after.filter((c) => c.author.email !== me && mentions(c, me) && !known.has(c.id));
}
