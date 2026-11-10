// The guests' suggestions waiting above a chat's composer (story 53): who suggests what, and the
// user's call on each. Send gives it to the agent as the user's own message; Edit puts it below
// the composer's draft, which then sends it; Dismiss drops it. The guests see what became of it.

import type { ChatInfo, Suggestion } from "@chauffeur/protocol";
import { ArrowUp, Pencil, Users, X } from "lucide-react";
import { useEffect, useState } from "react";
import { relativeTime } from "@/lib/format";
import { pendingSuggestions } from "@/lib/workspaceShare";
import { dismissSuggestion, editSuggestion, loadSuggestions, sendSuggestion, useWorkspaceShares } from "@/state/workspaceShares";
import { Button } from "../ui";

export function SuggestionCards({ chat }: { chat: ChatInfo }) {
  const suggestions = useWorkspaceShares((s) => s.suggestions[chat.id]);
  const editing = useWorkspaceShares((s) => s.editing[chat.id]);
  const [busy, setBusy] = useState<string | null>(null);
  useEffect(() => void loadSuggestions(chat.id), [chat.id]);
  const pending = pendingSuggestions(suggestions);
  if (pending.length === 0) return null;

  const send = async (suggestion: Suggestion) => {
    setBusy(suggestion.id);
    await sendSuggestion(chat, suggestion);
    setBusy(null);
  };
  const dismiss = async (suggestion: Suggestion) => {
    setBusy(suggestion.id);
    await dismissSuggestion(chat.id, suggestion);
    setBusy(null);
  };

  return (
    <div className="mx-auto w-full max-w-[860px] px-5 pb-1.5" data-testid="suggestions">
      <div className="rounded-lg border border-accent/30 bg-raised">
        <div className="flex h-7 items-center gap-2 border-b border-line px-2.5 text-[11.5px] text-faint">
          <Users size={12} className="text-accent" />
          <span>
            {pending.length === 1 ? "A guest suggests a message" : `${pending.length} suggestions from guests`} · nothing goes to the agent until you send it
          </span>
        </div>
        {pending.map((suggestion) => (
          <div key={suggestion.id} className="flex items-start gap-2 border-b border-line px-2.5 py-2 last:border-b-0" data-testid="suggestion">
            <div className="min-w-0 flex-1">
              <div className="text-[11.5px] text-muted" title={suggestion.author.email}>
                <span className="font-medium text-fg">{suggestion.author.name}</span> · {relativeTime(new Date(suggestion.at).toISOString())}
                {suggestion.id === editing && <span className="text-accent"> · in the composer: your next message sends it</span>}
              </div>
              <div className="whitespace-pre-wrap break-words text-[12.5px] text-fg selectable">{suggestion.text}</div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <Button size="sm" variant="primary" busy={busy === suggestion.id} onClick={() => void send(suggestion)} data-testid="suggestion-send" title="Send it to the agent as your message">
                <ArrowUp size={12} /> Send
              </Button>
              <Button
                size="sm"
                onClick={() => editSuggestion(chat.id, suggestion)}
                disabled={suggestion.id === editing}
                data-testid="suggestion-edit"
                title="Put it in the composer, to change it before sending"
              >
                <Pencil size={12} /> Edit
              </Button>
              <Button size="sm" variant="ghost" onClick={() => void dismiss(suggestion)} disabled={busy === suggestion.id} data-testid="suggestion-dismiss" title="Dismiss: nothing goes to the agent">
                <X size={12} />
              </Button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
