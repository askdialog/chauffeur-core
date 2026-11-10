# 0016. Keeping the machine awake while agents work

- **Date:** 2026-10-17
- **Status:** accepted

## Context

A Mac going to sleep suspends the agents running on it (asked by a user).

## Decision

Copied from `docs/SPEC.md`, where it was recorded as settled with Justin; the date of each part is in its text.

From `SPEC.md`, story 46:

> As a developer, I want my machine kept awake while agents work, so that a Mac going to sleep does not suspend them (asked by a user). `keep_awake` in settings.toml (Settings › Sleep): `never`; `while_working`, the default, no idle sleep while an agent works and 2 minutes after, the display may sleep and closing a MacBook's lid still sleeps it; `lid_closed` (macOS, opt-in), no sleep at all, the lid closed included. A discreet icon by the sidebar's settings button says when and why (`chauffeur power`). See **Sleep** (decided with Justin 2026-10-17).

From `SPEC.md`, Sleep:

> **Sleep (story 46).** An agent works while its chat has a turn starting or running (not one waiting for the user), background tasks running, or messages waiting for its workspace's creation; a workspace being created counts too. The daemon looks every 5 s and holds on 2 minutes after the work ends, to bridge the gap to the next turn. `while_working` holds a power assertion against idle system sleep: `caffeinate -i -w <daemon pid>` on macOS (simpler than IOKit through FFI, and it dies with the daemon, a crash or a kill included), `systemd-inhibit --what=idle:sleep` on Linux when logind is there (logged once otherwise). `lid_closed` adds `sudo -n /usr/bin/pmset -a disablesleep 1`, and `disablesleep 0` when the work stops. Choosing it installs, once, `/etc/sudoers.d/chauffeur-pmset` with exactly `<user> ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -a disablesleep 0` (the user name checked first: letters, digits, `_ - .`), mode 0440, root:wheel, written beside its place, checked by `visudo -cf` and renamed into place, or deleted; leaving the mode gives sleep back, then removes the file. Each asks for an administrator's password once, through `osascript … with administrator privileges` run by the daemon (it owns every write and runs in the user's session; the app's `settings.set` waits meanwhile); a prompt cancelled or failed leaves the setting as it was and says why. Safety nets: sleep comes back when the work stops, when the daemon stops, at its next start if a marker file in the state folder says it was left off (a crash, a kill), and on battery below `keep_awake_min_battery` (20%, read with `pmset -g batt`), turned off again above it or on AC; a `sudo -n` that fails (the rule missing) falls back to the assertion alone, with a warning in the sidebar's icon (decided with Justin 2026-10-17).

## Consequences

`while_working` is the default. `lid_closed` needs a narrowly scoped sudoers rule installed once with an administrator's password; sleep always comes back when the work stops, when the daemon stops or restarts, and on battery below `keep_awake_min_battery`.
