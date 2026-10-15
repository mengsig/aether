// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  RepositoryAliasCounts,
} from "../../protocol.ts";

/**
 * The answer to registering an alias, or to a dry run of it.
 */
export interface RepositoryAliasReport {
  "alias": string;
  "canonical": string;
  "counts": RepositoryAliasCounts;
  "dry_run": boolean;
  "existing": boolean;
}
