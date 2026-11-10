import type { AgentKind } from "@chauffeur/protocol";
import { useId } from "react";

/** Claude Code's pixel mascot in terracotta; Codex's prompt in a blue cloud. */
export function AgentIcon({ agent, size = 14, className }: { agent: AgentKind; size?: number; className?: string }) {
  return agent === "codex" ? <CodexIcon size={size} className={className} /> : <ClaudeIcon size={size} className={className} />;
}

function ClaudeIcon({ size, className }: { size: number; className?: string | undefined }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden shapeRendering="crispEdges">
      <g fill="#D97757">
        <rect x="2" y="4" width="12" height="6" />
        <rect x="0" y="6" width="2" height="2" />
        <rect x="14" y="6" width="2" height="2" />
        <rect x="3" y="10" width="1" height="3" />
        <rect x="5" y="10" width="1" height="3" />
        <rect x="10" y="10" width="1" height="3" />
        <rect x="12" y="10" width="1" height="3" />
      </g>
      <g fill="#1d1d21">
        <rect x="4" y="5" width="1" height="2" />
        <rect x="11" y="5" width="1" height="2" />
      </g>
    </svg>
  );
}

function CodexIcon({ size, className }: { size: number; className?: string | undefined }) {
  const gradient = useId();
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" className={className} aria-hidden>
      <defs>
        <linearGradient id={gradient} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#A7A5FF" />
          <stop offset="1" stopColor="#4F5BFF" />
        </linearGradient>
      </defs>
      <path
        fill={`url(#${gradient})`}
        d="M8 1.2c1.6 0 2.9.9 3.5 2.2 1.9.1 3.4 1.7 3.4 3.6 0 1-.4 1.9-1.1 2.6.2.4.3.8.3 1.3 0 1.8-1.5 3.3-3.3 3.3-.5 0-1-.1-1.4-.3-.4.5-1 .9-1.6.9H5.6C3.4 14.8 1.6 13 1.6 10.8c0-.8.2-1.6.7-2.2-.4-.6-.6-1.3-.6-2C1.7 4.6 3.2 3 5.1 3c.5-1.1 1.6-1.8 2.9-1.8Z"
      />
      <path d="M5.2 6.3 7 8 5.2 9.7" fill="none" stroke="white" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M8.3 10h2.6" stroke="white" strokeWidth="1.3" strokeLinecap="round" />
    </svg>
  );
}
