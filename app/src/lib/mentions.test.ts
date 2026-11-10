import { expect, test } from "bun:test";
import type { DesignComment, Mentioned } from "@chauffeur/protocol";
import { commentParam, excerpt, firstName, insertMention, mentionOptions, mentionQuery, mentions, mentionsIn, textParts, newMentions, threadParam, unreadCount, unreadThreads } from "./mentions";

const alice = { name: "Alice Martin", email: "alice@example.com" };
const bob = { name: "Bob", email: "bob@example.com" };
const carol = { name: "Carol", email: "carol@other.org" };
const people = [alice, bob, carol];

test("the query is the @word the caret ends, at a word's start only", () => {
  expect(mentionQuery("Hi @bo", 6)).toEqual({ start: 3, query: "bo" });
  expect(mentionQuery("@", 1)).toEqual({ start: 0, query: "" });
  expect(mentionQuery("(@al", 4)).toEqual({ start: 1, query: "al" });
  expect(mentionQuery("@bob@example.co", 15)).toEqual({ start: 0, query: "bob@example.co" });
  expect(mentionQuery("mail alice@x", 12)).toBeNull();
  expect(mentionQuery("@bob is", 7)).toBeNull();
  expect(mentionQuery("Hi @bo there", 6)).toEqual({ start: 3, query: "bo" });
});

test("the options match names and e-mails, never the writer", () => {
  expect(mentionOptions(people, "", { me: bob.email }).map((o) => o.person.email)).toEqual([alice.email, carol.email]);
  expect(mentionOptions(people, "mar", {}).map((o) => o.person.email)).toEqual([alice.email]);
  expect(mentionOptions(people, "car", {}).map((o) => o.person.email)).toEqual([carol.email]);
  // Starting with it first, containing it after.
  expect(mentionOptions(people, "c", {}).map((o) => o.person.email)).toEqual([carol.email, alice.email, bob.email]);
  expect(mentionOptions(people, "zed", {})).toEqual([]);
});

test("a full address no one has: of the team domain, or invited by the owner, else nothing", () => {
  expect(mentionOptions(people, "dan@example.com", { teamDomain: "example.com" })).toEqual([{ person: { email: "dan@example.com", name: "Dan" }, kind: "team" }]);
  expect(mentionOptions(people, "dan@else.io", { teamDomain: "example.com", canInvite: true })).toEqual([{ person: { email: "dan@else.io", name: "Dan" }, kind: "invite" }]);
  expect(mentionOptions(people, "dan@else.io", { teamDomain: "example.com" })).toEqual([]);
  // Someone who has access is offered as themselves.
  expect(mentionOptions(people, "bob@example.com", { canInvite: true }).map((o) => o.kind)).toEqual(["person"]);
  expect(firstName("jean-luc.picard@x.io")).toBe("Jean");
});

test("picking replaces the query with @Name and a space", () => {
  expect(insertMention("Hi @al", { start: 3, query: "al" }, 6, alice)).toEqual({ text: "Hi @Alice Martin ", caret: 17 });
  expect(insertMention("Hi @al there", { start: 3, query: "al" }, 6, alice)).toEqual({ text: "Hi @Alice Martin there", caret: 17 });
});

test("a mention survives as long as its @Name stays in the text", () => {
  const picked = [alice, bob];
  expect(mentionsIn("@Alice Martin and @Bob, look", picked)).toEqual([alice, bob]);
  expect(mentionsIn("@Alice Martin, look", picked)).toEqual([alice]);
  expect(mentionsIn("@Alice look", picked)).toEqual([]);
  expect(mentionsIn("@Bobby", picked)).toEqual([]);
  expect(mentionsIn("@Bob @Bob", [bob, bob])).toEqual([bob]);
});

test("the text is cut around its mentions, the longest name first", () => {
  const anne = { name: "Anne", email: "anne@x.io" };
  const anneMarie = { name: "Anne Marie", email: "am@x.io" };
  expect(textParts("Hey @Anne Marie and @Anne!", [anne, anneMarie])).toEqual([
    { text: "Hey " },
    { text: "@Anne Marie", mention: anneMarie },
    { text: " and " },
    { text: "@Anne", mention: anne },
    { text: "!" },
  ]);
  expect(textParts("No one", [anne])).toEqual([{ text: "No one" }]);
  expect(textParts("@Anne", undefined)).toEqual([{ text: "@Anne" }]);
});

const comment = (id: string, more: Partial<DesignComment> = {}): DesignComment => ({ id, screen: "home.html", author: alice, at: 1, text: "x", ...more });

test("unread mentions: the threads holding them, the count", () => {
  const comments = [comment("t1"), comment("r1", { reply_to: "t1" }), comment("t2"), comment("r2", { reply_to: "t2" })];
  expect(unreadThreads(comments, ["r2", "r1", "t1"])).toEqual(["t1", "t2"]);
  expect(unreadThreads(comments, [])).toEqual([]);
  expect(mentions(comment("t1", { mentions: [bob] }), bob.email)).toBe(true);
  expect(mentions(comment("t1"), bob.email)).toBe(false);
  const mention = (id: string, seen: boolean): Mentioned => ({
    id,
    place: "design",
    share_id: "s1",
    share_title: "Pricing",
    owner: alice,
    url: "https://x/s/s1?thread=t1",
    comment: "t1",
    thread: "t1",
    author: alice,
    text: "hi",
    at: 1,
    seen,
  });
  expect(unreadCount([mention("a", false), mention("b", true), mention("c", false)])).toBe(2);
});

test("a share page's link names the thread to open", () => {
  expect(threadParam("?thread=abc&screen=home.html")).toBe("abc");
  expect(threadParam("?screen=home.html")).toBeNull();
  expect(excerpt("a  b\n c")).toBe("a b c");
  expect(excerpt("abcdef", 4)).toBe("abc…");
});

test("a guest typing an address without access is told who to ask, and cannot pick it", () => {
  const people = [{ email: "alice@example.com", name: "Alice" }];
  expect(mentionOptions(people, "dan@else.io", { owner: "Alice" })).toEqual([{ person: { email: "dan@else.io", name: "Dan" }, kind: "no_access" }]);
  expect(mentionOptions(people, "dan@else.io", { owner: "Alice", canInvite: true }).map((o) => o.kind)).toEqual(["invite"]);
});

test("a workspace share's link names a chat and maybe a comment", () => {
  expect(commentParam("?chat=c1&comment=k1")).toEqual({ chat: "c1", comment: "k1" });
  expect(commentParam("?chat=c1")).toEqual({ chat: "c1", comment: null });
  expect(commentParam("?thread=t1")).toBeNull();
});

test("only a comment by someone else that names the reader for the first time is told", () => {
  const bob = { email: "bob@example.com", name: "Bob" };
  const me = { email: "alice@example.com", name: "Alice" };
  const comment = (id: string, author: typeof bob, named: string[]) => ({ id, author, mentions: named.map((email) => ({ email, name: "x" })) });
  const before = [comment("k1", bob, ["alice@example.com"])];
  const after = [...before, comment("k2", bob, ["alice@example.com"]), comment("k3", me, ["alice@example.com"]), comment("k4", bob, [])];
  expect(newMentions(before, after, me.email).map((c) => c.id)).toEqual(["k2"]);
});
