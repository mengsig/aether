// The pairing URL aetherd prints is `<origin>/pair#code=<code>` (crates/aether-remote/src/pairing.rs):
// the code rides in the fragment, so no server or proxy log ever sees it.

export const PAIR_ROUTE = "#/pair/aether";

const PAIR_PATH = /\/pair\/?$/;

interface PageLocation { readonly pathname: string; readonly hash: string }
interface PageHistory { replaceState(data: unknown, unused: string, url: string): void }

/** The code of an aetherd pairing URL, or null for any other page. */
export function readPairLink(location: PageLocation): string | null {
  if (!PAIR_PATH.test(location.pathname) || !location.hash.startsWith("#code=")) return null;
  const code = new URLSearchParams(location.hash.slice(1)).get("code")?.trim() ?? "";
  return code === "" ? null : code;
}

// The one code this tab was opened with, kept in memory only until a pairing starts.
let offered: string | null = null;

/**
 * Opens a pairing URL on the pairing screen: the code goes to {@link offeredPairCode} and the
 * address bar keeps only `/#/pair/aether`, so the code stays out of history. False for any other page.
 */
export function acceptPairLink(location: PageLocation, history: PageHistory): boolean {
  const code = readPairLink(location);
  if (code === null) return false;
  offered = code;
  // `./` from `/pair` is the site root, where the shell's relative files resolve.
  history.replaceState(null, "", `./${PAIR_ROUTE}`);
  return true;
}

/** The code a pairing URL brought for `daemon`, or "" when there is none. */
export function offeredPairCode(daemon: string): string {
  return daemon === "aether" && offered !== null ? offered : "";
}

/** Drops the offered code once a pairing has started with it. */
export function forgetPairCode(): void {
  offered = null;
}
