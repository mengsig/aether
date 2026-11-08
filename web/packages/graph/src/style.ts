import type { GNode } from "@aether-ui/protocol";
import { invariant } from "./invariant.ts";

export type StyledShape = "rounded-rect" | "circle" | "pill" | "diamond";
/** Tone is a semantic token name, e.g. accent, running, pr-merged or mastery-fragile. */
export interface NodeStyle { shape?: StyledShape; tone?: string; icon?: string }
export type NodeStyler = (node: GNode) => NodeStyle;
export interface GraphLegendEntry { label: string; tone?: string; status?: string; shape?: StyledShape; edge?: "solid" | "dashed" | "dotted" }

export function defaultNodeStyle(node: GNode): NodeStyle {
  const kind = node.group?.split(":")[0]?.toLowerCase();
  switch (kind) {
    case "agent": case "worker": return { shape: "pill", icon: "◉" };
    case "stage": case "reviewer": return { shape: "diamond", icon: "◇" };
    case "issue": return { shape: "circle", ...(node.status === undefined ? {} : { tone: node.status.startsWith("issue-") ? node.status : `issue-${node.status}` }) };
    case "proposition": return { shape: "circle" };
    case "pull_request": return { shape: "pill", icon: "⑂", ...(node.status === undefined ? {} : { tone: node.status.startsWith("pr-") ? node.status : `pr-${node.status}` }) };
    case "repo": case "initiative": return { shape: "rounded-rect", icon: "▤" };
    case "statement": return { shape: "pill" };
    default: return { shape: "rounded-rect" };
  }
}

export function toneToken(tone: string): string {
  invariant(tone.length > 0, "a graph tone names a token");
  invariant(/^[a-z][a-z0-9-]*$/.test(tone), "a graph tone is a semantic name, not a CSS value");
  if (/^(mastery|pr|issue)-/.test(tone)) return `--${tone}`;
  if (["accent", "success", "warning", "danger", "info"].includes(tone)) return `--color-${tone}`;
  return tone === "muted" ? "--color-text-muted" : `--status-${tone}`;
}

export function nodeScale(weight: number | undefined): number {
  if (weight === undefined) return 1;
  invariant(Number.isFinite(weight), "a node weight is finite");
  invariant(weight >= 0, "a node weight is nonnegative");
  return Math.max(0.85, Math.min(1.15, Math.sqrt(weight)));
}
