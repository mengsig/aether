// What an initiative page shows: its stored detail with each member's live state from core's items,
// and how far the members have got. Pure derivations over core's store.

import type { ArchonItem } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import type { Status } from "@aether-ui/ui";
import { invariant, isStateKind, isTerminal, statePill, type StateKind } from "../queue/model.ts";

export interface MemberRow {
  readonly id: string;
  readonly title: string;
  readonly kind: StateKind;
  readonly status: Status;
  readonly label: string;
  readonly spentUsd: number;
  readonly updatedAt: string;
}

export interface InitiativeView {
  readonly initiative: archon.Initiative;
  readonly members: readonly MemberRow[];
  readonly done: number;
  readonly failed: number;
  readonly open: number;
  readonly spentUsd: number;
}

const stateOf = (state: archon.QueueState | string): string => (typeof state === "string" ? state : state.state);

/** The detail core stored, with each member's live state where core's items hold it. */
export function initiativeView(detail: archon.InitiativeDetail, items: Readonly<Record<string, ArchonItem>>): InitiativeView {
  invariant(typeof detail.initiative?.id === "string", "an initiative detail names its initiative");
  invariant(Array.isArray(detail.members), "an initiative detail lists its members");
  const members = detail.members.map((member): MemberRow => {
    const live = items[member.id];
    const kind = live?.state ?? stateOf(member.state);
    invariant(isStateKind(kind), `member ${member.id} has a known state, not ${kind}`);
    const summary = live?.summary as Partial<archon.ItemSummary> | undefined;
    const pill = statePill(kind, false);
    return { id: member.id, title: member.title, kind, status: pill.status, label: pill.label, spentUsd: summary?.spent_usd ?? 0, updatedAt: summary?.updated_at ?? member.updated_at };
  });
  const done = members.filter((row) => row.kind === "succeeded").length;
  const failed = members.filter((row) => row.kind === "failed").length;
  const open = members.filter((row) => !isTerminal(row.kind)).length;
  const spentUsd = members.reduce((sum, row) => sum + row.spentUsd, 0);
  return { initiative: detail.initiative, members, done, failed, open, spentUsd };
}

const INITIATIVE_STATUS: Readonly<Record<string, Status>> = {
  open: "running", shipping: "dispatching", done: "succeeded", succeeded: "succeeded", failed: "failed", cancelled: "cancelled",
};

export function initiativePill(state: string): { status: Status; label: string } {
  const status = INITIATIVE_STATUS[state] ?? "queued";
  return { status, label: state.charAt(0).toUpperCase() + state.slice(1) };
}
