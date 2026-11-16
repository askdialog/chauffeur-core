# Update and restart in one step, at once or once no agent works

- **Date:** 2026-11-17
- **Status:** accepted

## Context

The update card took two steps: Update downloaded and installed the new version, then Restart reopened the app on it, warning when agents worked. Downloading and installing leave the agents working, so the first click only made the user wait and come back. Waiting for the agents to finish also meant watching them before clicking Restart. Justin asked for a "Restart when idle", as Conductor has, then for a single step.

## Decision

Decided with Justin on 2026-11-16 and 2026-11-17. The card offers one step that updates and restarts: **Update now**, or **When idle** (short, so that both hold on one line in the sidebar), beside the cross that hides the version. Either installs at once (`update.install`) and shows the progress with the restart that follows, with Cancel to drop that restart while the installation goes on. Once installed, Update now reopens the app at once. When idle reads `power.get` every 3 seconds, says what it waits for ("Restarts once no agent works: 2 agents working."), offers Cancel and Restart now, and restarts as soon as no chat works and no workspace is being created. This is the same count as the warning, so background tasks and the minutes the machine lingers awake do not hold it back. When agents work at the click, Update now warns before installing and offers Restart anyway, or When idle as the main button. A failed installation drops the planned restart.

## Consequences

- The plan lives in the app window, not the daemon: closing the app drops it, and the version stays installed. The restart is the app's own relaunch, so only a running app can do it.
- A version installed without a planned restart, from `chauffeur update --install` or a restart cancelled, keeps Restart and Restart when idle. The CLI keeps its two steps.
- Update now does not look at the agents again once the installation ends: an agent that started a turn during the download is interrupted, and resumes like any cut turn.
- In a browser the app cannot reopen itself: the card offers Update alone.
