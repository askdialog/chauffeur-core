// Compiles the sidecar into one executable, embedding the Claude Code binary of the target
// platform (the SDK cannot resolve its own from inside a compiled binary; see
// src/agents/claude/executable.ts).
//
//   bun scripts/build.ts [--target bun-darwin-arm64] [--outfile dist/chauffeur-sidecar]

import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { parseArgs } from "node:util";

const { values } = parseArgs({
  args: Bun.argv.slice(2),
  options: {
    target: { type: "string" },
    outfile: { type: "string", default: "dist/chauffeur-sidecar" },
  },
});

const root = join(dirname(import.meta.path), "..");
const [os, arch, libc] = platformOf(values.target);
const pkg = `@anthropic-ai/claude-agent-sdk-${os}-${arch}${libc ? `-${libc}` : ""}`;
const exe = os === "win32" ? "claude.exe" : "claude";
let claude: string;
try {
  claude = createRequire(join(root, "package.json")).resolve(`${pkg}/${exe}`);
} catch {
  console.error(`${pkg} is not installed: run \`bun install\` (cross-compiling needs \`bun add --force ${pkg}\`).`);
  process.exit(1);
}

const args = ["build", "src/main.ts", "--compile", "--asset", claude, "--outfile", values.outfile ?? "dist/chauffeur-sidecar"];
if (values.target) args.push(`--target=${values.target}`);
console.error(`embedding ${claude}`);
const build = Bun.spawnSync([process.execPath, ...args], { cwd: root, stdio: ["inherit", "inherit", "inherit"] });
process.exit(build.exitCode ?? 1);

/** `bun-darwin-arm64`, `bun-linux-x64-musl`… or the host platform. */
function platformOf(target: string | undefined): [string, string, string | undefined] {
  if (!target) return [process.platform, process.arch, undefined];
  const [, os, arch, libc] = target.split("-");
  if (!os || !arch) throw new Error(`unsupported target ${target}`);
  return [os === "windows" ? "win32" : os, arch, libc === "musl" ? "musl" : undefined];
}
