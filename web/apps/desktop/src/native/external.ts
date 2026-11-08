// Where a link the app window asks to open goes (`desktop.openExternal`). GitHub's own origins
// open in the browser; any other https: origin asks natively first, as nothing the renderer can
// write (remote.origins included) may vouch for a site; a loopback report opens in a report window.

import { BridgeRefusal } from "../security/validate.ts";

export type ExternalTarget =
  | { readonly kind: "browser"; readonly url: string; readonly origin: string; readonly trusted: boolean }
  | { readonly kind: "report"; readonly url: string; readonly origin: string };

/** Exact origins whose links open without asking: the GitHub pages the app links to. */
export const TRUSTED_ORIGINS: ReadonlySet<string> = new Set(["https://github.com", "https://www.github.com", "https://gist.github.com"]);
/** The hosts a local report server listens on, as the URL parser spells them. */
const LOOPBACK_HOSTS: ReadonlySet<string> = new Set(["127.0.0.1", "localhost", "[::1]"]);
export const MAX_EXTERNAL_URL = 2048;

function parse(value: string): URL {
  if (typeof value !== "string" || value === "") throw new BridgeRefusal("A link names a URL");
  if (value.length > MAX_EXTERNAL_URL) throw new BridgeRefusal(`A link is at most ${MAX_EXTERNAL_URL} characters`);
  const url = URL.parse(value);
  if (url === null) throw new BridgeRefusal("This link is not a valid URL");
  if (url.username !== "" || url.password !== "") throw new BridgeRefusal("Links with a user name or password are not opened");
  return url;
}

/** A loopback http: page with an explicit port: a report a local server shows. */
function reportTarget(url: URL, daemonReport: boolean): ExternalTarget {
  if (!LOOPBACK_HOSTS.has(url.hostname) && !daemonReport) {
    throw new BridgeRefusal(`Only https: links open from the app; ${url.host} is not on this computer`);
  }
  if (url.port === "" && !daemonReport) throw new BridgeRefusal("A local report link names its server's port");
  return { kind: "report", url: url.href, origin: url.origin };
}

/** The target of `value`; only main's own constant list makes a site trusted. */
export function externalTarget(value: string, daemonReport = false): ExternalTarget {
  const url = parse(value);
  if (url.protocol === "http:") return reportTarget(url, daemonReport);
  if (url.protocol !== "https:") throw new BridgeRefusal(`The app opens https: links only, not ${url.protocol}`);
  if (url.hostname === "") throw new BridgeRefusal("This link names no host");
  const trusted = TRUSTED_ORIGINS.has(url.origin);
  const target: ExternalTarget = { kind: "browser", url: url.href, origin: url.origin, trusted };
  if (!target.url.startsWith(`${target.origin}/`)) throw new Error(`${target.url} lies under its own origin`);
  return target;
}
