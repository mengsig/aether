// Whether a package build signs, decided from the standard electron-builder credentials alone.
// A build without them is unsigned, says so in its file names and metadata, and never signs
// with whatever identity the machine's keychain happens to hold.

export type Target = "linux" | "mac";

/** What the packaged app records about itself (package.json `aetherBuild`). */
export interface Signing {
  readonly signing: "signed" | "unsigned";
  readonly notarized: boolean;
}

const MAC_CERTIFICATE = ["CSC_LINK", "CSC_KEY_PASSWORD"] as const;
// The two credential sets electron-builder notarizes with: an Apple ID, or an App Store Connect key.
const NOTARY_APPLE_ID = ["APPLE_ID", "APPLE_APP_SPECIFIC_PASSWORD", "APPLE_TEAM_ID"] as const;
const NOTARY_API_KEY = ["APPLE_API_KEY", "APPLE_API_KEY_ID", "APPLE_API_ISSUER"] as const;

type Env = Readonly<Record<string, string | undefined>>;

function present(env: Env, names: readonly string[]): number {
  return names.filter((name) => (env[name] ?? "") !== "").length;
}

/** All of `names` set, none set, or an error naming the set: a half-configured secret is a mistake. */
function complete(env: Env, names: readonly string[]): boolean {
  const count = present(env, names);
  if (count !== 0 && count !== names.length) {
    throw new Error(`Signing credentials are incomplete: set all of ${names.join(", ")} or none of them`);
  }
  return count === names.length;
}

/**
 * The signing a build of `target` gets. electron-builder signs neither AppImage nor deb, so a
 * Linux build is always unsigned; a macOS build is signed only with a certificate, and notarized
 * only with one complete set of Apple credentials as well.
 */
export function signingFor(target: Target, env: Env): Signing {
  if (target !== "linux" && target !== "mac") throw new Error(`Unknown package target ${String(target)}`);
  if (target === "linux") return { signing: "unsigned", notarized: false };
  const certificate = complete(env, MAC_CERTIFICATE);
  const appleId = complete(env, NOTARY_APPLE_ID);
  const apiKey = complete(env, NOTARY_API_KEY);
  if (appleId && apiKey) throw new Error("Set one set of notarization credentials, not both");
  if (!certificate && (appleId || apiKey)) throw new Error("Notarization credentials need CSC_LINK and CSC_KEY_PASSWORD as well");
  return { signing: certificate ? "signed" : "unsigned", notarized: certificate && (appleId || apiKey) };
}

/**
 * electron-builder settings that make the build match `signing` exactly: an unsigned build skips
 * signing and notarization outright; a signed one fails rather than silently skip signing.
 */
export function signingConfig(target: Target, signing: Signing): Record<string, unknown> {
  if (signing.signing === "unsigned" && signing.notarized) throw new Error("an unsigned build is never notarized");
  if (target !== "mac") return {};
  if (signing.signing === "unsigned") return { mac: { identity: null, notarize: false } };
  return { forceCodeSigning: true, mac: { notarize: signing.notarized } };
}

/** The suffix every artifact name carries, so an unsigned file says so wherever it is copied. */
export function artifactSuffix(signing: Signing): string {
  return signing.signing === "unsigned" ? "unsigned" : signing.notarized ? "signed-notarized" : "signed";
}
