// What a build says about its own signing. The packaging script (build/package.ts) writes it
// into the packaged package.json as `aetherBuild`; a development run is always unsigned.

import type { BuildInfo } from "./updates.ts";

/** `manifest` is the app's package.json; `version` is app.getVersion(). */
export function buildInfo(manifest: unknown, version: string, isPackaged: boolean): BuildInfo {
  if (typeof version !== "string" || version === "") throw new Error("the app has a version");
  if (!isPackaged) return { version, signed: false, notarized: false, label: "unsigned build" };
  const record = (manifest as { aetherBuild?: unknown } | null)?.aetherBuild as
    { signing?: unknown; notarized?: unknown } | undefined;
  if (record === undefined || (record.signing !== "signed" && record.signing !== "unsigned")) {
    throw new Error("This package records no signing state: package it with build/package.ts");
  }
  if (typeof record.notarized !== "boolean") throw new Error("This package records no notarization state");
  const signed = record.signing === "signed";
  if (!signed && record.notarized) throw new Error("This package claims to be notarized but unsigned");
  return { version, signed, notarized: record.notarized, label: signed ? "signed build" : "unsigned build" };
}
