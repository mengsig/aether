// Apps launched from a dock or menu often get a PATH without the directories the installers put
// aether and archon in. The app appends those that exist, never ahead of the user's own PATH.

import { statSync } from "node:fs";
import { join, type Env, type Platform } from "../discovery/socket-paths.ts";

// What execvp searches when PATH is unset (glibc and macOS `_PATH_DEFPATH`).
const DEFAULT_PATH = "/usr/bin:/bin";

/** The standard install directories for `platform`, under `home`, in the order they are appended. */
export function installDirs(platform: Platform, home: string): string[] {
  if (!home.startsWith("/")) throw new Error(`the install directories sit under an absolute home, not ${home}`);
  const dirs = [join(home, ".local/bin"), join(home, ".cargo/bin"), "/usr/local/bin"];
  return platform === "darwin" ? [...dirs, "/opt/homebrew/bin"] : dirs;
}

/** `env` with each install directory that exists and is not on PATH yet appended to PATH. */
export function withInstallDirs(env: Env, platform: Platform, home: string, isDir: (dir: string) => boolean): Env {
  const current = env.PATH ?? DEFAULT_PATH;
  const present = new Set(current.split(":"));
  const added: string[] = [];
  for (const dir of installDirs(platform, home)) {
    if (present.has(dir) || !isDir(dir)) continue;
    present.add(dir);
    added.push(dir);
  }
  if (added.length === 0) return env;
  const next = [current, ...added].join(":");
  if (!next.startsWith(current)) throw new Error("install directories only ever follow the user's PATH");
  return { ...env, PATH: next };
}

/** Whether `dir` is a directory now; a missing one simply is not. */
export function isDirectory(dir: string): boolean {
  try {
    return statSync(dir).isDirectory();
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === "ENOENT" || code === "ENOTDIR" || code === "EACCES") return false;
    throw new Error(`Checking the install directory ${dir}`, { cause: error });
  }
}
