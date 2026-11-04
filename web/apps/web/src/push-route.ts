// Maps a daemon push's fragment to a route of this app. aetherd still writes the retired phone
// app's grammar (docs/remote.md); it names no connection, so agent notices open the inbox.

const AGENT_ID = /^agt_[0-9A-HJKMNP-TV-Z]{26}$/;

export function pushRoute(url: string | null | undefined): string | null {
  if (typeof url !== "string" || !url.startsWith("#")) return null;
  if (url.startsWith("#/")) return url;
  if (url === "#attention" || url.startsWith("#attention?")) return "#/inbox";
  if (url === "#activity") return "#/activity";
  if (url.startsWith("#agent/") && AGENT_ID.test(url.slice("#agent/".length))) return "#/inbox";
  return null;
}
