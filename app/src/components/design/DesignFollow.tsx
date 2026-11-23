import { Eye } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import type { DesignViewport } from "@chauffeur/protocol";
import { Button, Select } from "@/components/ui";

type Peer = { id: string; name: string; view: DesignViewport | null };
type Choice = { id: string; paused: boolean };

/** Only a local action can start or resume following. A returning peer never takes control. */
export function useDesignFollow(peers: readonly Peer[], onJoin?: () => void) {
  const [choice, setChoice] = useState<Choice | null>(null);
  const peer = peers.find((item) => item.id === choice?.id);
  useEffect(() => {
    if (choice && !peer) setChoice(null);
  }, [choice, peer]);

  const navigate = useCallback(() => {
    setChoice((current) => current && !current.paused ? { ...current, paused: true } : current);
  }, []);
  function join(id: string) {
    if (!peers.some((item) => item.id === id)) return;
    onJoin?.();
    setChoice({ id, paused: false });
  }
  return {
    peers, peer, paused: choice?.paused ?? false,
    remote: choice?.paused ? null : peer?.view,
    navigate, join, stop: () => setChoice(null),
  };
}

export type DesignFollow = ReturnType<typeof useDesignFollow>;

export function DesignFollowControls({ follow }: { follow: DesignFollow }) {
  const { peers, peer, paused } = follow;
  if (peer) {
    return (
      <div className="flex min-w-0 items-center gap-1 text-xs">
        <span role="status" className="truncate" title={peer.name}>
          {paused ? "You have control" : peer.view ? `Following ${peer.name}` : `Waiting for ${peer.name}’s view`}
        </span>
        {paused
          ? <Button size="sm" onClick={() => follow.join(peer.id)} title={`Resume following ${peer.name}`}>Resume following</Button>
          : <Button size="sm" onClick={follow.stop}>Stop</Button>}
        {paused && peers.length > 1 && <Button size="sm" variant="ghost" onClick={follow.stop}>Dismiss</Button>}
      </div>
    );
  }
  if (peers.length === 0) return null;
  if (peers.length === 1) {
    const [person] = peers;
    return (
      <Button size="sm" title={`Follow ${person.name}`} className="min-w-0 max-w-48" onClick={() => follow.join(person.id)}>
        <Eye size={13} className="shrink-0" /> <span className="truncate">Follow {person.name}</span>
      </Button>
    );
  }
  return <Select value="" title="Follow a participant" onChange={follow.join}
    shown={{ label: "Follow…" }} options={peers.map(({ id, name }) => ({ value: id, label: name }))} />;
}
