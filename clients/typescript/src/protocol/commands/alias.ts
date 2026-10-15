// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Make `alias`, an old repository id, resolve to the repository at
 * `root` or to the id `repository`, and move every memory filed under
 * the alias there. The move is one-way. Needs `repository_aliases`.
 */
export interface AliasRepositoryCommand {
  "cmd": "alias_repository";
  "alias": string;
  "dry_run"?: boolean;
  "repository"?: string | null;
  "root"?: string | null;
}

export function aliasRepository(args: Omit<AliasRepositoryCommand, "cmd">): AliasRepositoryCommand {
  return { cmd: "alias_repository", ...args };
}
