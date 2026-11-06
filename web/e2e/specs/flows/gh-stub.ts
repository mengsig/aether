// A `gh` for the GitHub flow, put first on the stack's PATH: it answers `gh api` reads of one fixture
// repository from files, records every call, and refuses anything else. It reaches no network.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";
import type { Layout } from "../../harness/env.ts";

const run = promisify(execFile);

export const OWNER_REPO = "e2e-owner/e2e-repo";
export const ORIGIN_URL = `https://github.com/${OWNER_REPO}.git`;

export interface GithubObjectFixture {
  readonly number: number;
  readonly title: string;
  readonly state: "open" | "closed";
  readonly body?: string;
  /** Set for a pull request; a merged one has `merged_at`. */
  readonly pull_request?: { readonly merged_at: string | null };
}

export interface GhStub {
  readonly log: string;
  /** Every argv `gh` was called with, in order. */
  calls(): Promise<string[][]>;
}

/** Answers `api --method GET ... repos/<OWNER_REPO>/issues/<n>` with `<dir>/<n>.http`; refuses the rest. */
function script(dir: string, log: string): string {
  return `#!/bin/sh
# e2e: a recording gh stub. It never runs the real gh and never opens a connection.
{ for arg in "$@"; do printf '%s\\037' "$arg"; done; printf '\\n'; } >> '${log}'
[ "$1" = api ] || { echo "gh stub: only gh api is served" >&2; exit 1; }
method=; last=
for arg in "$@"; do
  [ "$prev" = --method ] && method=$arg
  case "$arg" in -f|-F|--field|--raw-field|--input|-X*|--field=*|--raw-field=*|--input=*) echo "gh stub: refuses a write: $arg" >&2; exit 1 ;; esac
  prev=$arg; last=$arg
done
[ "$method" = GET ] || { echo "gh stub: only GET is served" >&2; exit 1; }
case "$last" in
  repos/${OWNER_REPO}/issues/[0-9]*) number=\${last##*/} ;;
  *) echo "gh stub: no fixture for $last" >&2; exit 1 ;;
esac
[ -f '${dir}'/"$number".http ] || { echo "gh stub: no fixture for #$number" >&2; exit 1; }
cat '${dir}'/"$number".http
`;
}

function http(object: GithubObjectFixture): string {
  const body = {
    number: object.number, title: object.title, state: object.state, state_reason: object.state === "closed" ? "completed" : null,
    labels: [{ name: "e2e" }], user: { login: "e2e-author" }, updated_at: "2026-11-01T12:00:00Z", body: object.body ?? "",
    ...(object.pull_request ? { draft: false, pull_request: object.pull_request } : {}),
  };
  return `HTTP/2.0 200 OK\r\nEtag: W/"e2e-${object.number}"\r\nX-Ratelimit-Remaining: 4999\r\n\r\n${JSON.stringify(body)}`;
}

/** Replaces the harness's refusing `gh` in `layout.bin` with the recording stub serving `objects`. */
export async function installGhStub(layout: Layout, objects: readonly GithubObjectFixture[]): Promise<GhStub> {
  assert.ok(objects.length > 0, "the stub serves at least one object");
  const dir = path.join(layout.root, "gh-fixtures");
  const log = path.join(layout.root, "gh-calls.log");
  await mkdir(dir, { recursive: true, mode: 0o700 });
  for (const object of objects) await writeFile(path.join(dir, `${object.number}.http`), http(object));
  await writeFile(log, "");
  const stub = path.join(layout.bin, "gh");
  const dirs = String(layout.env.PATH).split(":");
  assert.ok(dirs.slice(0, dirs.indexOf("/usr/local/bin")).includes(layout.bin), "the stack's PATH finds its own bin before the system's");
  await writeFile(stub, script(dir, log), { mode: 0o755 });
  return {
    log,
    async calls() {
      const text = await readFile(log, "utf8");
      return text.split("\n").filter(Boolean).map((line) => line.split("\u001f").slice(0, -1));
    },
  };
}

/** archond registers only a repository with a commit. fast-import takes the committer from its
 *  stream, so the fixture needs no git identity, configured or in the environment. */
async function firstCommit(repo: string, layout: Layout): Promise<void> {
  const readme = "# e2e-repo\n\nThe H4 GitHub flow's fixture repository.\n";
  const message = "Fixture repository for the GitHub flow\n";
  const stream = `commit refs/heads/main\ncommitter E2E fixture <e2e@example.invalid> 1762000000 +0000\n`
    + `data ${Buffer.byteLength(message)}\n${message}M 644 inline README.md\ndata ${Buffer.byteLength(readme)}\n${readme}\n`;
  const imported = run("git", ["-C", repo, "fast-import", "--quiet"], { env: layout.env, timeout: 10_000 });
  imported.child.stdin?.end(stream);
  await imported;
  await run("git", ["-C", repo, "reset", "--quiet", "--hard"], { env: layout.env, timeout: 10_000 });
  const { stdout } = await run("git", ["-C", repo, "rev-list", "--count", "HEAD"], { env: layout.env, timeout: 10_000 });
  assert.equal(stdout.trim(), "1", "the fixture has its one commit on main");
}

/** A git repository in the stack root whose `origin` names the fixture repository; it is never fetched. */
export async function fixtureRepository(layout: Layout): Promise<string> {
  const repo = path.join(layout.root, "work", "e2e-repo");
  await mkdir(repo, { recursive: true, mode: 0o700 });
  const git = (...args: string[]) => run("git", ["-C", repo, ...args], { env: layout.env, timeout: 10_000 });
  await git("init", "--quiet", "--initial-branch=main");
  await firstCommit(repo, layout);
  await git("remote", "add", "origin", ORIGIN_URL);
  const { stdout } = await git("remote", "get-url", "origin");
  assert.equal(stdout.trim(), ORIGIN_URL, "the fixture's origin names the fixture repository");
  return repo;
}
