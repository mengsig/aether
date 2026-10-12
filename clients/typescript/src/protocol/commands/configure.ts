// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  GithubWatcherConfig,
  RemoteSettings,
} from "../../protocol.ts";

/**
 * Replace watcher settings only if no other operator changed them.
 */
export interface ConfigureGithubWatcherCommand {
  "cmd": "configure_github_watcher";
  "config": GithubWatcherConfig;
  "expected_revision": number;
}

export function configureGithubWatcher(args: Omit<ConfigureGithubWatcherCommand, "cmd">): ConfigureGithubWatcherCommand {
  return { cmd: "configure_github_watcher", ...args };
}

/**
 * Replace the remote settings only if nobody else changed them. Opens
 * or closes the listener at once. Host only.
 */
export interface ConfigureRemoteCommand {
  "cmd": "configure_remote";
  "expected_revision": number;
  "settings": RemoteSettings;
}

export function configureRemote(args: Omit<ConfigureRemoteCommand, "cmd">): ConfigureRemoteCommand {
  return { cmd: "configure_remote", ...args };
}
