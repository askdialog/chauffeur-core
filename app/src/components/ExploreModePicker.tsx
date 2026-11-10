// An exploration's mode, where a workspace's composer has its permission picker: read-only, and
// the way out of it, Continue as workspace, which asks first as the header's button does.

import { GitBranch, Lock } from "lucide-react";
import { useState } from "react";
import { promoteUnavailable } from "@/lib/promote";
import { useApp } from "@/state/app";
import { PromoteDialog } from "./PromoteDialog";
import { ReadOnlyMode, Select, type SelectOption } from "./ui";

type Choice = "read_only" | "promote";

export function ExploreModePicker({ workspaceId }: { workspaceId: string }) {
  const workspace = useApp((s) => s.workspaces[workspaceId]);
  const [promoting, setPromoting] = useState(false);
  if (workspace?.state !== "ready") return <ReadOnlyMode />;
  const options: Array<SelectOption<Choice>> = [
    {
      value: "read_only",
      label: "Read-only",
      description: "The agents read and explain the code",
      icon: <Lock size={13} className="shrink-0" />,
    },
    {
      value: "promote",
      label: "Continue as workspace…",
      description: "Branch, setup, ports: the agents can edit",
      icon: <GitBranch size={13} className="shrink-0" />,
      unavailable: promoteUnavailable(workspace),
    },
  ];
  return (
    <>
      <Select
        value="read_only"
        options={options}
        onChange={() => setPromoting(true)}
        title="An exploration's agents read and explain: they cannot edit, commit, push or open pull requests"
        testId="read-only-mode"
        width={320}
      />
      <PromoteDialog open={promoting} onClose={() => setPromoting(false)} workspace={workspace} />
    </>
  );
}
