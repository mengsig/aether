// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Refresh stale quota observations through explicitly enabled probe turns.
 */
export interface RefreshProviderUsageCommand {
  "cmd": "refresh_provider_usage";
  "provider"?: string | null;
}

export function refreshProviderUsage(args: Omit<RefreshProviderUsageCommand, "cmd">): RefreshProviderUsageCommand {
  return { cmd: "refresh_provider_usage", ...args };
}
