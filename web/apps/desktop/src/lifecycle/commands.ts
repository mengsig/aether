// Runs one of the CLIs to completion for setup: never through a shell, with a deadline, bounded
// output, an explicit environment and the user's home as its directory.

import { execFile } from "node:child_process";
import path from "node:path";
import { BridgeRefusal } from "../security/validate.ts";
import type { ToolContext } from "./managed-start.ts";

export type ToolName = "aether" | "archon";

/** How a command ended: not installed, or ran and exited with its output. */
export type CommandResult =
  | { readonly found: false }
  | { readonly found: true; readonly code: number; readonly stdout: string; readonly stderr: string };

export type CommandRunner = (tool: ToolName, args: readonly string[], timeoutMs: number) => Promise<CommandResult>;

const MAX_OUTPUT_BYTES = 1024 * 1024;
const MAX_DETAIL_CHARS = 300;

/** The first non-empty line of `text`, trimmed and cut short, for a message the user reads. */
export function firstLine(text: string): string {
  const line = text.split("\n").map((part) => part.trim()).find((part) => part !== "") ?? "";
  const chars = [...line];
  return chars.length <= MAX_DETAIL_CHARS ? line : `${chars.slice(0, MAX_DETAIL_CHARS).join("")}…`;
}

/** A runner over the real CLIs in `context`; a timeout or a failure to run is a plain refusal. */
export function commandRunner(context: ToolContext): CommandRunner {
  if (!path.isAbsolute(context.cwd)) throw new Error("the CLIs run in an absolute directory");
  return (tool, args, timeoutMs) => {
    if (tool !== "aether" && tool !== "archon") throw new Error(`setup runs only aether and archon, not ${String(tool)}`);
    if (!(timeoutMs > 0)) throw new Error("a setup command has a positive deadline");
    return new Promise((resolve, reject) => {
      const options = {
        cwd: context.cwd, env: { ...context.env }, timeout: timeoutMs, maxBuffer: MAX_OUTPUT_BYTES,
        encoding: "utf8" as const, windowsHide: true,
      };
      execFile(tool, [...args], options, (error, stdout, stderr) => {
        if (error === null) return resolve({ found: true, code: 0, stdout, stderr });
        if (error.code === "ENOENT") return resolve({ found: false });
        if (error.killed === true) {
          return reject(new BridgeRefusal(`${tool} did not answer within ${Math.round(timeoutMs / 1000)} seconds. Try again in a moment.`));
        }
        if (typeof error.code === "number") return resolve({ found: true, code: error.code, stdout, stderr });
        reject(new BridgeRefusal(`${tool} could not run (${String(error.code ?? error.message)}).`, { cause: error }));
      });
    });
  };
}
