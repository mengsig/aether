// The setup's first step: whether aether and archon are installed where the app can run them,
// and which version each is, from each CLI's own version command.

import type { SetupTool } from "@aether-ui/protocol";
import type { CommandRunner, ToolName } from "./commands.ts";

export const TOOLS: readonly ToolName[] = ["aether", "archon"];

/** `aether version` prints `aether 1.3.0 (protocol 24)`; `archon --version` prints `archon 1.4.12`. */
const VERSION_ARGS: Readonly<Record<ToolName, readonly string[]>> = {
  aether: ["version"],
  archon: ["--version"],
};

export const VERSION_TIMEOUT_MS = 10_000;
const VERSION = /^[0-9A-Za-z.+-]{1,64}$/;

/** The version in a CLI's version line, or undefined when the line is not one. */
export function parseVersion(tool: ToolName, stdout: string): string | undefined {
  if (!(tool in VERSION_ARGS)) throw new Error(`no version command for ${String(tool)}`);
  const line = stdout.trim().split("\n")[0] ?? "";
  const words = line.split(/\s+/);
  if (words[0] !== tool || words[1] === undefined) return undefined;
  if (tool === "aether" && words.length > 2 && !/^\(protocol \d+\)$/.test(words.slice(2).join(" "))) return undefined;
  if (tool === "archon" && words.length !== 2) return undefined;
  return VERSION.test(words[1]) ? words[1] : undefined;
}

/** One tool as its version command finds it; a tool that runs but says no version is still found. */
async function findTool(run: CommandRunner, tool: ToolName): Promise<SetupTool> {
  const result = await run(tool, VERSION_ARGS[tool], VERSION_TIMEOUT_MS);
  if (!result.found) return { name: tool, found: false };
  const version = result.code === 0 ? parseVersion(tool, result.stdout) : undefined;
  return version === undefined ? { name: tool, found: true } : { name: tool, found: true, version };
}

/** Both tools, in TOOLS order, checked side by side. */
export async function findTools(run: CommandRunner): Promise<SetupTool[]> {
  const tools = await Promise.all(TOOLS.map((tool) => findTool(run, tool)));
  if (tools.length !== TOOLS.length || tools.some((tool, index) => tool.name !== TOOLS[index])) {
    throw new Error("one answer per tool, in order");
  }
  return tools;
}
