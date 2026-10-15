// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * What registering an alias moves, or moved: every memory under the alias,
 * in any state, and the agents whose stored scope still names it.
 */
export interface RepositoryAliasCounts {
  "agents": number;
  "duplicates": number;
  "repository": number;
  "repository_role": number;
}
