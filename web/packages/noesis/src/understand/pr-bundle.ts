// A pull request's bundle, from the GitHub objects archond cached for its repository
// (`list_github_objects`). The PR's text is untrusted, so all of it stays preliminary or less.

import type { aether, archon } from "@aether-ui/protocol";
import { invariant } from "../learn/model.ts";
import { bundleOf } from "./bundle.ts";
import { attested, prStanding, type Standing } from "./archon-bundles.ts";

export interface PrId {
  readonly host: string;
  /** `owner/name`, lowercased as Archon stores it. */
  readonly repo: string;
  readonly number: number;
}

const PR_ID = /^([a-z0-9](?:[a-z0-9.-]*[a-z0-9])?(?::\d{1,5})?)\/([\w.-]+)\/([\w.-]+)(?:#|\/)(\d{1,9})$/i;

/** `<host>/<owner>/<repo>#<number>` (Archon's `github:` key without its prefix) or `<host>/<owner>/<repo>/<number>`. */
export function parsePrId(id: string): PrId | null {
  const match = PR_ID.exec(id.trim());
  if (!match?.[1] || !match[2] || !match[3] || !match[4]) return null;
  const number = Number(match[4]);
  if (!Number.isSafeInteger(number) || number < 1) return null;
  return { host: match[1].toLowerCase(), repo: `${match[2]}/${match[3]}`.toLowerCase(), number };
}

/** The cached object for this PR among a `github_objects` reply's objects, or null. */
export function findPr(objects: readonly archon.GithubObject[], pr: PrId): archon.GithubObject | null {
  invariant(pr.number > 0 && pr.repo.includes("/"), "a PR id names a repository and a number");
  const found = objects.filter((object) => object.ref.kind === "pull_request" && object.ref.number === pr.number
    && object.ref.repo.toLowerCase() === pr.repo && object.ref.host.toLowerCase() === pr.host);
  invariant(found.length <= 1, `archond caches ${pr.repo}#${pr.number} once`);
  return found[0] ?? null;
}

function checksEvidence(ref: aether.EvidenceRef, checks: archon.ChecksSummary): aether.EvidenceItem {
  const passed = checks.total > 0 && checks.failed === 0 && checks.state === "success";
  const words = passed ? `All ${checks.total} checks passed` : `Checks ${checks.state}: ${checks.failed} of ${checks.total} failing`;
  const standing: Standing = passed
    ? { status: "verified", basis: "Verified: every check run passed." }
    : { status: "preliminary", basis: "Preliminary: the checks have not all passed." };
  return attested(ref, standing, words);
}

/** The PR's state and review, and its checks as a separate fact a check run established. */
export function prBundle(object: archon.GithubObject): aether.EvidenceBundle {
  const { ref } = object;
  invariant(ref.kind === "pull_request", "a PR bundle reads a pull request");
  invariant(Number.isSafeInteger(ref.number) && ref.number > 0, "a pull request has a number");
  const id = `${ref.host}/${ref.repo}#${ref.number}`;
  const url = /^[a-z0-9.-]+$/i.test(ref.host) ? `https://${ref.host}/${ref.repo}/pull/${ref.number}` : null;
  const evidenceRef = { kind: "pr", id, url } as const;
  const state = object.state === "open" && object.draft ? "draft" : object.state;
  const facts = [
    `Pull request #${ref.number} in ${ref.repo}: ${object.title}`,
    `State: ${state}${object.review_decision ? ` · review ${object.review_decision.replace("_", " ")}` : ""}`,
    object.author ? `Author: ${object.author}` : null,
    object.labels.length > 0 ? `Labels: ${object.labels.join(", ")}` : null,
  ].filter((line): line is string => line !== null);
  const items = [attested(evidenceRef, prStanding(object.state, object.review_decision), facts.join("\n"))];
  if (object.checks) items.push(checksEvidence(evidenceRef, object.checks));
  return bundleOf(`#${ref.number} ${object.title}`, items);
}
