import { PanelLeftOpen } from "lucide-react";
import { kbd } from "@/lib/keys";
import { toggleSidebar, useApp } from "@/state/app";
import { IconButton } from "./ui";

/** Brings back the sidebar once it is hidden; nothing while it shows. */
export function ShowSidebarButton({ className }: { className?: string }) {
  const hidden = useApp((s) => s.sidebarHidden);
  if (!hidden) return null;
  return (
    <IconButton title={`Show the sidebar (${kbd("⌘B")})`} onClick={() => toggleSidebar(false)} className={className}>
      <PanelLeftOpen size={14} />
    </IconButton>
  );
}
