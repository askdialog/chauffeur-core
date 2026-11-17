# A delegated chat closes once it has answered, unless the user took part in it

- **Date:** 2026-11-17
- **Status:** accepted

## Context

A delegated chat ([0047](0047-delegated-chats.md)) is a tab of its own, and stays one after its answer has reached its parent. Its answer is in the parent's `delegate` card already, so a finished task like "Feasibility study" leaves a tab that serves nothing, and with a few delegations the tab strip fills up. Some users follow their delegated chats and want them kept. Justin asked for the tabs to go away as a subagent does once it is done.

## Decision

Once a delegated chat's answer has been given to its parent (the `delegate` call's result, or the message sent once the parent's turn had ended), Chauffeur closes it, as the user's Close does: out of the tabs, its conversation kept, its agent session stopped, listed among the closed chats and reopened from there, with ⇧⌘T or from the parent's card. It stays open when the user took part in it: they wrote in it, it asked them a question or a permission, or they looked at it while it worked. It also stays open when its turn did not end with an answer (an error, an interrupt): that is where one reads why. Closing is Chauffeur's rule, not an instruction to the agent. Decided with Justin 2026-11-17.

## Consequences

- The daemon closes the chat; the agent is told nothing and does nothing. More work for the same delegated chat (`delegate` with `chat`) reopens it, as any message to a closed chat does.
- The daemon learns that the user looked at a delegated chat while it worked from the app: a new call, `chat.seen`, sent when the app shows a delegated chat that works or waits for the user.
- Which delegated chats the user took part in is kept in the daemon's memory: an answer is given within the daemon's run, and a daemon that stops loses the waiter that would close the chat anyway.
- No setting: the user who follows a delegated chat keeps it by looking at it.
