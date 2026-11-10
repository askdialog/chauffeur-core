# Confirming managed process exits before handoff

The source admission fence prevents new launches. It does not prove that any
existing process has stopped. The daemon now has internal checked stop operations
for its registered scripts and terminals; no public RPC or transfer coordinator
invokes them yet.

`ProcessManager::stop_and_wait` sends TERM to a live managed script group, waits
for the supplied grace, then escalates to KILL if its leader is still live. It
requires a successful child reap and an absent process group before returning the
retained process observation. A leader that exited with a surviving group is an
explicit error: the historical group is inspected, never signalled. The workspace
variant attempts every registered script before returning an error.

Signalling and reaping share the child-handle mutex. The child watcher polls
`try_wait` every 25 ms; the signal path checks the child under that same lock, so
its PID cannot have been reaped and reused between the check and the signal.
Existing ordinary script stops use this identity guard too. Wait failures remain
unknown/running and are logged, rather than publishing a successful exit.

`TerminalManager::close_and_wait` requests stop, awaits successful child reaping,
and only then removes the handle. A kill error, failed watcher or timeout retains
the terminal for reconciliation/retry. The terminal's child handle similarly
serializes reaping and signals. The ordinary close path remains immediate after
a successful stop request; it is not a handoff confirmation API.

These are bounded, in-memory managed-child confirmations, not source-stop
receipts. Terminal exit does not prove its descendants exited. Scripts may have
detached descendants outside their original group, and a restarted daemon no
longer has these child handles. A process group could also be reused after its
leader exited: a positive existence probe is conservatively an error. Caller
cancellation does not undo signals already sent and must leave the durable fence
installed. Observation requires the caller to install/drain that fence first;
these methods alone do not prevent concurrent admissions.

A handoff coordinator still must authenticate the exact operation, reconcile all
launch intents including uncertain sidecar responses, stop native agent sessions
with verified termination, fence external writers/services and database exports,
and produce authenticated source-stop evidence. Nothing here advances the cloud
journal, clears a fence, starts a worker or authorizes a transfer.

Real disposable-daemon tests cover TERM-resistant script escalation, successful
script/terminal reaping, retained script observations, unknown IDs, and a leader
that leaves a background process alive. In that last case, confirmation fails and
the background process is demonstrably still alive; the test then cleans it up.
