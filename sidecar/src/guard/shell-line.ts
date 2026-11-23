// Reads a shell command line as the shell does, for a guard that judges what each command of it
// does: a word keeps what its quotes meant, a redirection is told apart from a quoted `>`, and the
// commands inside a word (`$( … )`, backticks, `<( … )`), a group or a here-document are read
// too. It reads, it never runs: a variable it does not see set, or a command's output, stays a
// part of unknown value, for the guard to judge.

/** A piece of a word: text, a variable, a command whose output lands there, or a value unknown. */
export type Part =
  | { text: string; glob: boolean }
  /** `$NAME`, `${NAME}`, `$1`, `~` (HOME); `${NAME:-word}` falls back on `word`'s parts. */
  | { variable: string; fallback?: Part[] }
  | { command: List }
  /** `name=( … )`: its words. */
  | { array: Word[] }
  | { unknown: true };

export interface Word {
  parts: Part[];
}

export interface Redirection {
  /** The target is written (`>`, `>>`, `>|`, `&>`, `<>`, `>&file`), not only read. */
  writes: boolean;
  /** `<<< word`: the word is the command's input, not a file. */
  input?: boolean;
  target: Word;
}

export interface Command {
  words: Word[];
  redirections: Redirection[];
  /** The commands an unquoted here-document's text, or an arithmetic `(( … ))`, runs. */
  here: List[];
  /** Its here-documents' text: its input. */
  documents: string[];
}

export interface Group {
  group: List;
  /** `( … )` runs in a subshell: a `cd` in it stays there. */
  subshell: boolean;
  redirections: Redirection[];
}

/** A command or a group, with the operator after it: `&&`, `||`, `;`, `;;`, `&`, `|`, `|&`, a newline, or none. */
export type List = { node: Command | Group; then: string }[];

/** The commands of a command line. */
export function readLine(line: string): List {
  return new Reader(line, 0).list(false);
}

/** The text of a word, its variables and commands left out. */
function wordText(word: Word): string {
  return word.parts.map((part) => ("text" in part ? part.text : "")).join("");
}

/** Text added to a word: joined to the text before it unless either is a wildcard. */
function addText(parts: Part[], value: string, glob = false): void {
  const last = parts.at(-1);
  if (last && "text" in last && !glob && !last.glob) last.text += value;
  else parts.push({ text: value, glob });
}

const OPERATORS = ["&&", "||", ";;", "|&", ";", "|", "&", "\n"];
const REDIRECTIONS = ["&>>", "&>", ">>", ">|", ">&", "<<<", "<<-", "<<", "<>", "<&", ">", "<"];
/** Past this depth of nested commands, a word's command is a value unknown. */
const MAX_DEPTH = 20;

interface HereDocument {
  delimiter: string;
  tabs: boolean;
  expands: boolean;
  command: Command;
}

class Reader {
  private i = 0;
  private readonly pending: HereDocument[] = [];

  constructor(
    private readonly s: string,
    private readonly depth: number,
  ) {}

  /** Commands up to the end, or up to the `)` closing a group or a substitution (left for the caller). */
  list(inParentheses: boolean): List {
    const items: List = [];
    for (;;) {
      this.blanks();
      if (this.i >= this.s.length || (inParentheses && this.peek() === ")")) return items;
      const node = this.node();
      const then = this.operator();
      if (node) items.push({ node, then });
      // A `)` with no group open: passed over.
      else if (!then) this.i++;
    }
  }

  private peek(ahead = 0): string {
    return this.s.charAt(this.i + ahead);
  }

  private at(text: string): boolean {
    return this.s.startsWith(text, this.i);
  }

  private blanks(): void {
    for (;;) {
      const c = this.peek();
      if (c === " " || c === "\t" || c === "\r") this.i++;
      else if (c === "\\" && this.peek(1) === "\n") this.i += 2;
      else return;
    }
  }

  private operator(): string {
    const operator = OPERATORS.find((op) => this.at(op)) ?? "";
    this.i += operator.length;
    if (operator === "\n") this.hereDocuments();
    return operator;
  }

  private node(): Command | Group | undefined {
    if (this.at("((")) {
      const start = this.i;
      this.skipParentheses();
      const inner = this.s.slice(start + 2, this.i - 2);
      this.i = start;
      // `((cd x && y))` is no arithmetic: the shell reads it as groups.
      if (!/&&|\|\||;|\|/.test(inner)) return { words: [], redirections: [], here: this.arithmetic(), documents: [] };
    }
    if (this.peek() === "(") {
      this.i++;
      const group = this.list(true);
      if (this.peek() === ")") this.i++;
      const command = this.command();
      return { group, subshell: true, redirections: command?.redirections ?? [] };
    }
    return this.command();
  }

  private command(): Command | undefined {
    const command: Command = { words: [], redirections: [], here: [], documents: [] };
    // Inside `[[ … ]]`, `<` and `>` compare strings.
    let test = false;
    for (;;) {
      this.blanks();
      const c = this.peek();
      if (c === "" || c === "\n" || c === ";" || c === "|" || c === ")" || (c === "&" && this.peek(1) !== ">")) break;
      if (c === "#") {
        while (this.i < this.s.length && this.peek() !== "\n") this.i++;
        break;
      }
      if (c === "(") {
        // `name() { … }` defines a function: its body is read as commands.
        this.i++;
        this.blanks();
        if (this.peek() === ")") this.i++;
        continue;
      }
      if (!test && this.redirectionAhead()) {
        this.redirection(command);
        continue;
      }
      const word = this.word(test);
      if (!word) {
        this.i++;
        continue;
      }
      command.words.push(word);
      const text = wordText(word);
      if (command.words.length === 1 && text === "[[") test = true;
      else if (text === "]]") test = false;
    }
    return command.words.length > 0 || command.redirections.length > 0 || command.documents.length > 0 ? command : undefined;
  }

  /** A redirection starts here: `>`, `2>`, `&>`, `<`…, not a process substitution `<( … )`. */
  private redirectionAhead(): boolean {
    let j = this.i;
    while (/\d/.test(this.s.charAt(j))) j++;
    const c = this.s.charAt(j);
    if ((c === "<" || c === ">") && this.s.charAt(j + 1) === "(") return false;
    return c === "<" || c === ">" || (j === this.i && this.at("&>"));
  }

  private redirection(command: Command): void {
    while (/\d/.test(this.peek())) this.i++;
    const operator = REDIRECTIONS.find((op) => this.at(op)) ?? ">";
    this.i += operator.length;
    this.blanks();
    const target = this.word(false) ?? { parts: [] };
    if (operator === "<<" || operator === "<<-") {
      const quoted = this.lastWordQuoted;
      this.pending.push({ delimiter: wordText(target), tabs: operator === "<<-", expands: !quoted, command });
      return;
    }
    // `>&2`, `2>&1`, `>&-`: a stream copied or closed, no file.
    if ((operator === ">&" || operator === "<&") && /^(?:\d+|-)$/.test(wordText(target))) return;
    const writes = operator !== "<" && operator !== "<<<" && operator !== "<&";
    command.redirections.push({ writes, target, ...(operator === "<<<" ? { input: true } : {}) });
  }

  /** After a newline, the text of the here-documents opened on its line, up to their delimiters. */
  private hereDocuments(): void {
    for (const doc of this.pending.splice(0)) {
      let body = "";
      while (this.i < this.s.length) {
        const end = this.s.indexOf("\n", this.i);
        const line = this.s.slice(this.i, end < 0 ? this.s.length : end);
        this.i = end < 0 ? this.s.length : end + 1;
        if ((doc.tabs ? line.replace(/^\t+/, "") : line) === doc.delimiter) break;
        body += `${line}\n`;
      }
      doc.command.documents.push(body);
      if (!doc.expands) continue;
      const parts: Part[] = [];
      new Reader(body, this.depth + 1).doubleQuoted(parts, "");
      for (const part of parts) if ("command" in part) doc.command.here.push(part.command);
    }
  }

  private lastWordQuoted = false;

  private word(test: boolean): Word | undefined {
    const start = this.i;
    const parts: Part[] = [];
    this.lastWordQuoted = false;
    for (;;) {
      const c = this.peek();
      if (c === "" || c === " " || c === "\t" || c === "\r" || c === "\n" || c === ";" || c === "|" || c === "&" || c === ")") break;
      if (c === "(") {
        const before = parts.at(-1);
        if (before && "text" in before && before.text.endsWith("=")) {
          this.i++;
          parts.push({ array: this.array() });
          continue;
        }
        // `@( … )`, `!( … )`… patterns: one value, the commands in it run.
        if (before && "text" in before && /[@!+*?]$/.test(before.text)) {
          for (const command of this.arithmetic(1)) parts.push({ command });
          parts.push({ unknown: true });
          continue;
        }
        break;
      }
      if (c === "<" || c === ">") {
        if (this.peek(1) === "(") {
          this.i += 2;
          parts.push({ command: this.nested() });
          continue;
        }
        if (!test) break;
        addText(parts, c);
        this.i++;
        continue;
      }
      if (c === "\\") {
        const next = this.peek(1);
        this.i += 2;
        if (next !== "\n") addText(parts, next);
        continue;
      }
      if (c === "'") {
        this.lastWordQuoted = true;
        const end = this.s.indexOf("'", this.i + 1);
        const stop = end < 0 ? this.s.length : end;
        addText(parts, this.s.slice(this.i + 1, stop));
        this.i = stop + 1;
        continue;
      }
      if (c === "$" && this.peek(1) === "'") {
        this.lastWordQuoted = true;
        this.i += 2;
        let value = "";
        while (this.i < this.s.length && this.peek() !== "'") {
          if (this.peek() === "\\") this.i++;
          value += this.peek();
          this.i++;
        }
        this.i++;
        addText(parts, value);
        continue;
      }
      if (c === '"') {
        this.lastWordQuoted = true;
        this.i++;
        this.doubleQuoted(parts, '"');
        continue;
      }
      if (c === "$") {
        this.dollar(parts);
        continue;
      }
      if (c === "`") {
        this.backtick(parts);
        continue;
      }
      if (c === "~" && this.i === start) {
        const user = /^~([A-Za-z0-9._-]*)/.exec(this.s.slice(this.i))?.[1] ?? "";
        this.i += 1 + user.length;
        parts.push(user ? { unknown: true } : { variable: "HOME" });
        continue;
      }
      addText(parts, c, "*?[{".includes(c));
      this.i++;
    }
    return this.i === start ? undefined : { parts };
  }

  /** Inside double quotes (or a here-document's text, with no closing quote): variables and commands. */
  doubleQuoted(parts: Part[], closing: string): void {
    while (this.i < this.s.length) {
      const c = this.peek();
      if (closing && c === closing) {
        this.i++;
        return;
      }
      if (c === "\\" && "$`\"\\\n".includes(this.peek(1)) && this.peek(1) !== "") {
        if (this.peek(1) !== "\n") addText(parts, this.peek(1));
        this.i += 2;
      } else if (c === "$") this.dollar(parts);
      else if (c === "`") this.backtick(parts);
      else {
        addText(parts, c);
        this.i++;
      }
    }
  }

  private dollar(parts: Part[]): void {
    if (this.at("$((")) {
      this.i++;
      for (const command of this.arithmetic()) parts.push({ command });
      parts.push({ unknown: true });
      return;
    }
    if (this.at("$(")) {
      this.i += 2;
      parts.push({ command: this.nested() });
      return;
    }
    if (this.at("${")) {
      const inside = this.braces();
      const name = /^(?:[A-Za-z_][A-Za-z0-9_]*|\d+)$/.exec(inside)?.[0];
      if (name) {
        parts.push({ variable: name });
        return;
      }
      const inner: Part[] = [];
      // `${x:-word}`, `${x-word}`, `${x:=word}`: `word` when `x` is unset.
      const fallback = /^([A-Za-z_][A-Za-z0-9_]*|\d+):?[-=]([\s\S]*)$/.exec(inside);
      new Reader(fallback?.[2] ?? inside, this.depth + 1).doubleQuoted(inner, "");
      if (fallback?.[1]) {
        parts.push({ variable: fallback[1], fallback: inner });
        return;
      }
      // Any other form: its commands run, its value is unknown.
      parts.push(...inner.filter((part) => "command" in part), { unknown: true });
      return;
    }
    const name = /^\$([A-Za-z_][A-Za-z0-9_]*|\d)/.exec(this.s.slice(this.i))?.[1];
    if (name) {
      this.i += 1 + name.length;
      parts.push({ variable: name });
      return;
    }
    if (/[@*#?$!-]/.test(this.peek(1))) {
      this.i += 2;
      parts.push({ unknown: true });
      return;
    }
    this.i++;
    parts.push({ text: "$", glob: false });
  }

  private backtick(parts: Part[]): void {
    let j = this.i + 1;
    let inner = "";
    while (j < this.s.length && this.s.charAt(j) !== "`") {
      if (this.s.charAt(j) === "\\" && "`$\\".includes(this.s.charAt(j + 1)) && this.s.charAt(j + 1) !== "") j++;
      inner += this.s.charAt(j);
      j++;
    }
    this.i = j + 1;
    parts.push(this.depth < MAX_DEPTH ? { command: new Reader(inner, this.depth + 1).list(false) } : { unknown: true });
  }

  /** The commands up to the `)` closing a `$(`, `<(` or `>(` already passed. */
  private nested(): List {
    const list = this.list(true);
    if (this.peek() === ")") this.i++;
    return list;
  }

  /** `${ … }`'s text, up to its closing brace: quotes, escapes and nested braces passed over. */
  private braces(): string {
    const start = this.i + 2;
    let depth = 1;
    let quote = "";
    let j = start;
    for (; j < this.s.length; j++) {
      const c = this.s.charAt(j);
      if (c === "\\") j++;
      else if (quote) {
        if (c === quote) quote = "";
      } else if (c === "'" || c === '"') quote = c;
      // Only a nested `${` opens a brace: `${x:-{}` ends at its first `}`.
      else if (c === "$" && this.s.charAt(j + 1) === "{") {
        depth++;
        j++;
      } else if (c === "}" && --depth === 0) break;
    }
    this.i = Math.min(j + 1, this.s.length);
    return this.s.slice(start, j);
  }

  /** The words of an array `( … )`, the `(` passed. */
  private array(): Word[] {
    const words: Word[] = [];
    for (;;) {
      this.blanks();
      while (this.peek() === "\n") this.i++;
      const c = this.peek();
      if (c === "" || c === ")") {
        this.i++;
        return words;
      }
      const word = this.word(false);
      if (word) words.push(word);
      else this.i++;
    }
  }

  /** An arithmetic `(( … ))` (or a pattern's `( … )`, `open` 1) from its first `(`: the commands substituted in it. */
  private arithmetic(open = 2): List[] {
    const start = this.i;
    this.skipParentheses();
    const parts: Part[] = [];
    if (this.depth < MAX_DEPTH) new Reader(this.s.slice(start + open, this.i - open), this.depth + 1).doubleQuoted(parts, "");
    return parts.flatMap((part) => ("command" in part ? [part.command] : []));
  }

  /** Passes over `(` … its matching `)`, quotes included: arithmetic, arrays, patterns. */
  private skipParentheses(): void {
    let depth = 0;
    let quote = "";
    for (; this.i < this.s.length; this.i++) {
      const c = this.peek();
      if (quote) {
        if (c === quote) quote = "";
      } else if (c === "'" || c === '"') quote = c;
      else if (c === "(") depth++;
      else if (c === ")" && --depth === 0) {
        this.i++;
        return;
      }
    }
  }
}
