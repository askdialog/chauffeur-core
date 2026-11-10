# Coding standards

How code is written in this repository. They are the judgement calls a reviewer makes. What `cargo fmt`, `cargo clippy` and `tsc` enforce stays in their configuration, and how to work in the repo (commands, tests, branches) stays in `CLAUDE.md`.

Each rule is one the code already follows. When a file disagrees with a rule, the file is what changes the next time it is touched, not a reason to break the rule again.

## Shared

### Simplicity

Write the simplest code that solves the problem: no abstraction, option or helper for a need nobody has yet. Prefer a direct expression to a wrapper or a single-use variable. Keep a function to one responsibility, return early to reduce nesting.

### Comments

A comment states what the code cannot: a constraint, an invariant, a workaround, the reason for a choice. Never narrate a step (`// fetch the workspaces`) or restate a name.

- A file or module may open with a comment (`//!` in Rust, `//` in TypeScript) saying what it is for and what it deliberately does not do.
- A public function or exported type gets a doc comment (`///`, `/** */`) when its name and signature do not say enough: the case it handles, what it returns when nothing matches, what it refuses.
- Decisions settled with Justin, and their dates, live in `docs/SPEC.md`, not in code. A comment may say what the SPEC requires, not who asked.

### Messages the user reads

An error or a refusal says what failed and what to do instead, in one sentence, with the name of the thing.

```ts
toast(`Could not pin it: ${errorMessage(error)}`, "error");
```

```ts
export const READ_ONLY_DENIAL =
  "This Chauffeur workspace is an exploration, read-only: commits, pushes, ... are refused. " +
  "To change code, the user creates a Chauffeur workspace.";
```

### Tests

A test is named for the behaviour it proves, as a sentence, not for the function it calls.

```rust
fn blocks_do_not_overlap_and_are_stable_per_owner() { ... }
```

```ts
test("an unknown chat falls back on the workspace's activity", () => { ... });
```

A change of behaviour comes with the test that fails without it. Tests never touch the real `~/.chauffeur` or `~/chauffeur` (see `CLAUDE.md`).

## Rust

### Errors

A request handler of the daemon returns `crate::error::Result`. Pick the variant for what the client should do about it, not for where it failed: `NotFound`, `Conflict` (with `ConflictWith` when the client needs data to act), `BadRequest`, `Unavailable`. Anything unexpected is `Internal`: let `?` convert an `anyhow::Error` and add `.context("what we were doing")`.

Each variant becomes a JSON-RPC code in `Error::to_rpc`; a new kind of failure a client must tell apart gets a new variant and a code in `chauffeur-protocol`, not a string to parse.

### Panics

No `unwrap()` on a value that can legitimately be absent or fail. Two accepted forms:

- `mutex.lock().unwrap()` on a `std::sync::Mutex`: a poisoned lock means another thread already panicked.
- `.expect("<the invariant>")` on what cannot fail, naming why (`.expect("events serialize")`, `.expect("valid regex")`).

### Logging

`tracing`, with the error and the context as fields and a lowercase message saying what did not happen. No `println!` in the daemon.

```rust
tracing::warn!(%error, agent = agent.as_str(), "could not list the agent's models");
```
### Lints

`cargo clippy --all-targets` stays free of warnings. An `#[allow(...)]` is a last resort and carries a comment with the reason.

### Tests

Unit tests sit in a `#[cfg(test)] mod tests` at the bottom of the file they test. What needs the daemon running or several crates goes in the crate's `tests/`. A store test uses `Store::open_in_memory()`.

## TypeScript (app and sidecar)

### Types

`strict` is on and stays on: no `any`, no `@ts-ignore`; an `as` cast needs a reason a type guard could not give. The sidecar also runs with `noUncheckedIndexedAccess`: handle the missing element rather than asserting it.

Types that cross the wire come from `@chauffeur/protocol`; never declare them again by hand.

### Imports

`import type` for types (`verbatimModuleSyntax`), the `@/` alias in the app for anything outside the current folder.

### App

- Logic with no React in it lives in `app/src/lib/`, in its own file with a `.test.ts` beside it (`attention.ts`). A component holds layout and event wiring.
- A call to the daemon is `client.call("<method>", params)`, typed by the protocol. An action several components share lives in `app/src/state/` (`pinWorkspace`); a call used by one component stays in it.
- A call that can fail catches its error and tells the user with a toast. When the screen changed before the answer, the catch puts it back (`foldSection`).
- An empty `.catch(() => {})` is allowed only for a call whose failure changes nothing for the user, such as marking a workspace seen.

### Sidecar

A guard (`sidecar/src/guard/`) refuses with a message that names the allowed way, as `READ_ONLY_DENIAL` does. A guard has a test listing what it must allow and what it must refuse.
