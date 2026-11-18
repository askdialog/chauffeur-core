import { useGitlabStatus } from "@/state/gitlab";
import { GithubSetup } from "./GithubSetup";
import { GitlabSetup } from "./GitlabSetup";

/** The setup of the forge the installation goes through: GitHub's checklist, or GitLab's when the
 *  installation is set to it (docs/adr/gitlab-as-the-forge.md). */
export function ForgeSetup({ compact = false, localReposSelected = false }: { compact?: boolean; localReposSelected?: boolean }) {
  const { status } = useGitlabStatus();
  if (status?.enabled) return <GitlabSetup compact={compact} />;
  return <GithubSetup compact={compact} localReposSelected={localReposSelected} />;
}
