import type { AgentKind } from "@chauffeur/protocol";
import { Paperclip, Plug, Plus } from "lucide-react";
import { useId, useRef, useState } from "react";
import { mcpSetupCommand, SLACK_MCP_URL } from "@/lib/connectors";
import { AGENT_NAMES } from "@/lib/models";
import { requestTerminal } from "@/state/app";
import { Button, IconButton, inputClass, Modal, Popover } from "../ui";
import { AttachmentPicker } from "./ImageAttachments";

const itemClass = "flex h-7 items-center gap-2 rounded-md px-2 text-left text-fg hover:bg-hover focus-visible:bg-hover";

export function ComposerAddMenu({ workspaceId, agent, slackSuggested, sessionStarted, disabled, onFiles }: {
  workspaceId: string;
  agent: AgentKind;
  slackSuggested: boolean;
  sessionStarted: boolean;
  disabled?: boolean;
  onFiles: (files: File[]) => void;
}) {
  const anchor = useRef<HTMLButtonElement>(null);
  const [menu, setMenu] = useState(false);
  const [setup, setSetup] = useState(false);
  const closeMenu = () => {
    setMenu(false);
    anchor.current?.focus();
  };

  return (
    <AttachmentPicker onFiles={onFiles}>
      {(openPicker) => (
        <>
          <IconButton
            ref={anchor}
            type="button"
            title="Add to chat"
            aria-expanded={menu}
            disabled={disabled}
            onClick={() => setMenu(!menu)}
          >
            <Plus size={14} />
          </IconButton>
          {menu && (
            <Popover anchor={anchor} width={182} height={68} onClose={closeMenu} className="p-1">
              <button type="button" data-autofocus className={itemClass} onClick={() => { closeMenu(); openPicker(); }}>
                <Paperclip size={14} /> Attach files…
              </button>
              <button type="button" className={itemClass} onClick={() => { closeMenu(); setSetup(true); }}>
                <Plug size={14} /> Add connector…
              </button>
            </Popover>
          )}
          {setup && (
            <ConnectorSetup
              key={agent}
              agent={agent}
              slackSuggested={slackSuggested}
              sessionStarted={sessionStarted}
              onClose={() => setSetup(false)}
              onSetup={(command) => {
                setSetup(false);
                requestTerminal(workspaceId, command);
              }}
            />
          )}
        </>
      )}
    </AttachmentPicker>
  );
}

function ConnectorSetup({ agent, slackSuggested, sessionStarted, onClose, onSetup }: {
  agent: AgentKind;
  slackSuggested: boolean;
  sessionStarted: boolean;
  onClose: () => void;
  onSetup: (command: string) => void;
}) {
  const form = useId();
  const [name, setName] = useState(slackSuggested ? "slack" : "");
  const [url, setUrl] = useState(slackSuggested ? SLACK_MCP_URL : "");
  const [error, setError] = useState<string | null>(null);

  return (
    <Modal
      open
      width={420}
      title="Add connector"
      onClose={onClose}
      footer={(
        <>
          <Button type="button" onClick={onClose}>Cancel</Button>
          <Button type="submit" form={form} variant="primary" disabled={!name.trim() || !url.trim()}>Set up in terminal</Button>
        </>
      )}
    >
      <form id={form} className="space-y-3 text-[12px] text-muted" onSubmit={(event) => {
        event.preventDefault();
        const result = mcpSetupCommand(agent, name, url);
        if ("error" in result) setError(result.error);
        else onSetup(result.command);
      }}>
        <p>{AGENT_NAMES[agent]} adds the MCP server to your user settings. Finish any sign-in in the terminal or browser.</p>
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" size="sm" onClick={() => { setName("slack"); setUrl(SLACK_MCP_URL); setError(null); }}>Slack</Button>
          {slackSuggested && <span>Suggested for the link in your draft.</span>}
        </div>
        {agent === "vibe" && url.trim() === SLACK_MCP_URL && (
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" size="sm" onClick={() => onSetup("vibe")}>Open Vibe</Button>
            <span>For Mistral’s Slack integration, run <code>/connectors</code> in Vibe.</span>
          </div>
        )}
        <label className="block space-y-1">
          <span className="block font-medium text-fg">Name</span>
          <input className={inputClass} autoFocus value={name} onChange={(event) => { setName(event.target.value); setError(null); }} placeholder="slack" spellCheck={false} />
        </label>
        <label className="block space-y-1">
          <span className="block font-medium text-fg">Server URL</span>
          <input className={inputClass} value={url} onChange={(event) => { setUrl(event.target.value); setError(null); }} placeholder="https://example.com/mcp" spellCheck={false} />
        </label>
        {error && <p role="alert" className="text-danger">{error}</p>}
        {sessionStarted && <p>After setup, close and reopen this chat to load the connector with your next message.</p>}
      </form>
    </Modal>
  );
}
