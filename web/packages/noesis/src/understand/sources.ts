// One bundle source per entity kind the sheet can learn from; a new kind is one entry here.

import type { UnderstandKind } from "./bundle.ts";
import { useAgentSource } from "./agent-source.ts";
import { useItemSource, usePullRequestSource, useRunSource } from "./archon-sources.ts";
import type { BundleSource } from "./read.ts";

export const SOURCES: Readonly<Record<UnderstandKind, BundleSource>> = {
  item: useItemSource,
  run: useRunSource,
  agent: useAgentSource,
  pull_request: usePullRequestSource,
};
