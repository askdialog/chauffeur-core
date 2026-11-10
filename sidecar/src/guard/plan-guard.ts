// The one place a confined Claude session may write. An exploration's: its plans folder,
// `<workspace>/.chauffeur/plans` (Claude Code's plan mode asks the model to write its plan to a
// file). A design chat's: its design folder, `<workspace>/.chauffeur/designs/<chat>`, where it
// writes its screens and canvas.json. The repos stay read-only either way.
//
// This is the enforcement for the file tools (Write, Edit, MultiEdit): Claude Code's OS sandbox
// covers Bash only. It runs as a PreToolUse hook, before any permission rule or prompt, and again
// in canUseTool. A path is accepted only when it names a regular file, or a file still to be
// created, whose folder resolves (symlinks followed) inside the real write folder, itself a real
// folder in a real parent folder. Everything else is refused with the reason.

import { lstatSync, realpathSync, type Stats } from "node:fs";
import { dirname, isAbsolute, resolve, sep } from "node:path";

/** The tools that write a file named by their input: refused anywhere but the write folder. */
export const PLAN_WRITE_TOOLS = ["Write", "Edit", "MultiEdit"];

/** File-writing tools a confined session never gets, wherever they point. */
export const NEVER_WRITE_TOOLS = ["NotebookEdit"];

/** True for the tools the guard looks at. */
export function writesFiles(toolName: string): boolean {
  return PLAN_WRITE_TOOLS.includes(toolName) || NEVER_WRITE_TOOLS.includes(toolName);
}

export const NO_PLANS_DENIAL =
  "This Chauffeur workspace is an exploration, read-only, and has no plans folder: no file can be written. " +
  "Read and explain; to change code, the user creates a Chauffeur workspace.";

/** Which folder a confined session writes into: an exploration's plans, or a design chat's screens. */
export type WriteFolderKind = "plans" | "design";

const FOLDER_NAME: Record<WriteFolderKind, string> = { plans: "the plans folder", design: "the design folder" };
const SESSION_NAME: Record<WriteFolderKind, string> = { plans: "an exploration", design: "a design chat" };

export function planGuardDenial(plansDir: string, why: string): string {
  return (
    `This Chauffeur workspace is an exploration, read-only: the only files you may write are your plans, ` +
    `in ${plansDir}/. Refused: ${why}. Read and explain; to change code, the user creates a Chauffeur workspace.`
  );
}

export function designGuardDenial(designDir: string, why: string): string {
  return (
    "This is a Chauffeur design chat: you design screens, you never change code. The only files you may write " +
    `are your screens and canvas.json, in ${designDir}/. Refused: ${why}. When the design is ready, the user ` +
    "sends it to a coding chat that implements it."
  );
}

/**
 * Why the tool call writes outside the write folder (`kind` names it), or undefined when it may go
 * on (it writes a file of the folder, or it is not a file-writing tool). `cwd` resolves a relative
 * path, as Claude Code does.
 */
export function planWriteRefusal(
  toolName: string,
  toolInput: unknown,
  plansDir: string,
  cwd: string,
  kind: WriteFolderKind = "plans",
): string | undefined {
  if (NEVER_WRITE_TOOLS.includes(toolName)) return `${toolName} is not available in ${SESSION_NAME[kind]}`;
  if (!PLAN_WRITE_TOOLS.includes(toolName)) return undefined;
  const filePath = (toolInput as { file_path?: unknown } | null)?.file_path;
  return planPathRefusal(filePath, plansDir, cwd, kind);
}

/** Why `filePath` is not a file of the write folder (`kind` names it), or undefined when it is. */
export function planPathRefusal(filePath: unknown, plansDir: string, cwd: string, kind: WriteFolderKind = "plans"): string | undefined {
  if (typeof filePath !== "string" || filePath.trim() === "") return "no file path";
  if (filePath.includes("\0")) return "a NUL byte in the path";
  if (filePath.includes("\\")) return `a backslash in ${filePath}`;
  if (filePath.startsWith("~")) return `${filePath} is under the home folder`;
  if (filePath.split("/").includes("..")) return `\`..\` in ${filePath}`;
  if (filePath.endsWith("/") || filePath.endsWith("/.")) return `${filePath} names a folder`;
  if (!isAbsolute(plansDir)) return `${FOLDER_NAME[kind]} is unknown`;
  const realPlans = realPlansDir(plansDir);
  if (typeof realPlans !== "string") return realPlans.refused;
  if (!isAbsolute(filePath) && !isAbsolute(cwd)) return `${filePath} is relative`;
  const target = resolve(cwd, filePath);

  let realParent: string;
  try {
    realParent = realpathSync.native(dirname(target));
  } catch {
    return `the folder of ${target} does not exist`;
  }
  if (realParent !== realPlans && !realParent.startsWith(realPlans + sep)) {
    return `${target} is outside ${FOLDER_NAME[kind]}`;
  }

  const stats = lstatOrMissing(target);
  if (stats === "missing") return undefined;
  if (stats === "error") return `${target} cannot be examined`;
  if (stats.isSymbolicLink()) return `${target} is a symbolic link`;
  if (!stats.isFile()) return `${target} is not a regular file`;
  // A hard link shares its content with a file elsewhere, perhaps in a repo.
  if (stats.nlink > 1) return `${target} is a hard link`;
  return undefined;
}

/**
 * The write folder's real path, refused when it, or the folder holding it (the notes folder for
 * plans, `.chauffeur/designs` for a design chat), is not a real folder.
 */
function realPlansDir(plansDir: string): string | { refused: string } {
  const plans = resolve(plansDir);
  for (const dir of [dirname(plans), plans]) {
    const stats = lstatOrMissing(dir);
    if (stats === "missing") return { refused: `the folder ${dir} is missing` };
    if (stats === "error") return { refused: `the folder ${dir} cannot be examined` };
    if (stats.isSymbolicLink()) return { refused: `the folder ${dir} is a symbolic link` };
    if (!stats.isDirectory()) return { refused: `${dir} is not a folder` };
  }
  try {
    return realpathSync.native(plans);
  } catch {
    return { refused: `the folder ${plans} cannot be resolved` };
  }
}

function lstatOrMissing(path: string): Stats | "missing" | "error" {
  try {
    return lstatSync(path);
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "ENOENT" ? "missing" : "error";
  }
}
