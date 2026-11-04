// The packaging setup: signing follows the credentials and is never faked, every artifact name
// and the packaged metadata say when a build is unsigned, updates have one source, and the
// shipped dependency tree keeps the licence rule.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { test } from "node:test";
import { DEV_ONLY_EXCEPTIONS, licenseProblems, type Exception } from "../../../../scripts/licenses.ts";
import { OUTPUT_DIR, overrides, workspaceVersion } from "../../build/package.ts";
import { artifactSuffix, signingConfig, signingFor, type Signing } from "../../build/signing.ts";
import { DESKTOP_ROOT, REPO_ROOT } from "../../src/main/paths.ts";
import { DEEP_LINK_SCHEME } from "../../src/native/deep-links.ts";
import { UPDATE_FEED } from "../../src/update/updates.ts";

// electron-updater parses its own update files with js-yaml, so it is always installed beside it.
const yaml = createRequire(import.meta.url)("js-yaml") as { load(text: string): unknown };

interface BuilderConfig {
  directories: { output: string };
  files: string[];
  extraResources: Array<{ from: string; to: string }>;
  artifactName: string;
  publish: Record<string, string>;
  protocols: Array<{ schemes: string[] }>;
  linux: { target: string[]; executableName: string };
  mac: { target: Array<{ target: string }> };
  electronFuses: Record<string, boolean>;
}

const CONFIG = yaml.load(readFileSync(path.join(DESKTOP_ROOT, "electron-builder.yml"), "utf8")) as BuilderConfig;

const CERTIFICATE = { CSC_LINK: "base64-p12", CSC_KEY_PASSWORD: "secret" };
const APPLE_ID = { APPLE_ID: "a@example.com", APPLE_APP_SPECIFIC_PASSWORD: "pw", APPLE_TEAM_ID: "TEAM" };
const API_KEY = { APPLE_API_KEY: "/k.p8", APPLE_API_KEY_ID: "id", APPLE_API_ISSUER: "issuer" };

test("a build is signed only with a complete certificate, and Linux packages never are", () => {
  const unsigned: Signing = { signing: "unsigned", notarized: false };
  assert.deepEqual(signingFor("mac", {}), unsigned);
  assert.deepEqual(signingFor("linux", { ...CERTIFICATE, ...APPLE_ID }), unsigned, "electron-builder signs no AppImage or deb");
  assert.deepEqual(signingFor("mac", CERTIFICATE), { signing: "signed", notarized: false });
  assert.deepEqual(signingFor("mac", { ...CERTIFICATE, ...APPLE_ID }), { signing: "signed", notarized: true });
  assert.deepEqual(signingFor("mac", { ...CERTIFICATE, ...API_KEY }), { signing: "signed", notarized: true });
  assert.deepEqual(signingFor("mac", { CSC_LINK: "", CSC_KEY_PASSWORD: "" }), unsigned, "empty secrets are absent");
  assert.throws(() => signingFor("mac", { CSC_LINK: "p12" }), /incomplete: set all of CSC_LINK, CSC_KEY_PASSWORD/);
  assert.throws(() => signingFor("mac", { ...CERTIFICATE, APPLE_ID: "a" }), /incomplete/);
  assert.throws(() => signingFor("mac", APPLE_ID), /need CSC_LINK/);
  assert.throws(() => signingFor("mac", { ...CERTIFICATE, ...APPLE_ID, ...API_KEY }), /not both/);
});

test("an unsigned build skips signing outright and a signed one may not skip it", () => {
  assert.deepEqual(signingConfig("mac", { signing: "unsigned", notarized: false }), { mac: { identity: null, notarize: false } });
  assert.deepEqual(signingConfig("mac", { signing: "signed", notarized: false }), { forceCodeSigning: true, mac: { notarize: false } });
  assert.deepEqual(signingConfig("mac", { signing: "signed", notarized: true }), { forceCodeSigning: true, mac: { notarize: true } });
  assert.deepEqual(signingConfig("linux", { signing: "unsigned", notarized: false }), {});
  assert.throws(() => signingConfig("mac", { signing: "unsigned", notarized: true }), /never notarized/);
});

test("every artifact name and the packaged metadata carry the signing", () => {
  assert.equal(CONFIG.artifactName, "${productName}-${version}-${os}-${arch}-${env.AETHER_DESKTOP_SIGNING}.${ext}");
  assert.equal(artifactSuffix({ signing: "unsigned", notarized: false }), "unsigned");
  assert.equal(artifactSuffix({ signing: "signed", notarized: false }), "signed");
  assert.equal(artifactSuffix({ signing: "signed", notarized: true }), "signed-notarized");
  assert.deepEqual(overrides("linux", "1.2.11", { signing: "unsigned", notarized: false }), {
    extraMetadata: { version: "1.2.11", aetherBuild: { signing: "unsigned", notarized: false } },
  });
});

test("packages carry the workspace version, which release tags name", () => {
  const cargo = readFileSync(path.join(REPO_ROOT, "Cargo.toml"), "utf8");
  assert.match(workspaceVersion(cargo), /^\d+\.\d+\.\d+$/);
  assert.equal(workspaceVersion('[package]\nversion = "9.9.9"\n[workspace.package]\nedition = "2024"\nversion = "1.2.3"\n'), "1.2.3");
  assert.throws(() => workspaceVersion('[package]\nversion = "1.0.0"\n'), /no \[workspace.package\]/);
  assert.throws(() => workspaceVersion('[workspace.package]\n[other]\nversion = "1.0.0"\n'), /names no release version/);
});

test("updates come only from this repository's GitHub releases", () => {
  assert.deepEqual(CONFIG.publish, { ...UPDATE_FEED, releaseType: "release" });
  const repository = /^repository = "https:\/\/github.com\/([^/]+)\/([^/"]+)"$/m.exec(readFileSync(path.join(REPO_ROOT, "Cargo.toml"), "utf8"));
  assert.deepEqual([repository?.[1], repository?.[2]], [UPDATE_FEED.owner, UPDATE_FEED.repo]);
});

test("the packages are AppImage, deb, dmg and zip, built outside the desktop package", () => {
  assert.deepEqual(CONFIG.linux.target, ["AppImage", "deb"]);
  assert.deepEqual(CONFIG.mac.target.map((entry) => entry.target), ["dmg", "zip"]);
  assert.notEqual(CONFIG.linux.executableName, "aether", "the daemon's command keeps its name");
  assert.equal(path.resolve(DESKTOP_ROOT, CONFIG.directories.output), OUTPUT_DIR);
  assert.ok(!OUTPUT_DIR.startsWith(DESKTOP_ROOT + path.sep));
  assert.deepEqual(CONFIG.files, ["package.json", "src/**"], "no test, build script or credential is packaged");
  assert.deepEqual(CONFIG.protocols.flatMap((entry) => entry.schemes), [DEEP_LINK_SCHEME]);
  assert.deepEqual(CONFIG.electronFuses, {
    runAsNode: false, enableNodeOptionsEnvironmentVariable: false, enableNodeCliInspectArguments: false, onlyLoadAppFromAsar: true,
  });
});

test("the package carries the committed web/dist and both schemas where paths.ts looks", () => {
  const resources = Object.fromEntries(CONFIG.extraResources.map((entry) => [entry.to, path.resolve(DESKTOP_ROOT, entry.from)]));
  assert.deepEqual(resources, {
    "web/dist": path.join(REPO_ROOT, "web", "dist"),
    "schemas/protocol.schema.json": path.join(REPO_ROOT, "clients", "protocol.schema.json"),
    "schemas/archon.protocol.schema.json": path.join(REPO_ROOT, "web", "schemas", "archon.protocol.schema.json"),
  });
});

// electron-builder's own tree, which never ships: the app holds only `dependencies`. minimatch is
// the web workspace's own exception; the rest are packages only electron-builder pulls in.
const BUILD_TOOL_EXCEPTIONS: Readonly<Record<string, Exception>> = {
  minimatch: DEV_ONLY_EXCEPTIONS.minimatch as Exception,
  ...Object.fromEntries(["chownr", "isexe", "minipass", "tar", "yallist"].map((name) =>
    [name, { license: "BlueOak-1.0.0", reason: "permissive; tar and node-gyp in electron-builder's tree" }])),
  "truncate-utf8-bytes": { license: "WTFPL", reason: "permissive; electron-builder's file-name sanitizer" },
};

test("the shipped dependencies are MIT, ISC, BSD or Apache-2.0, and build tools stay dev-only", () => {
  const lock = JSON.parse(readFileSync(path.join(DESKTOP_ROOT, "package-lock.json"), "utf8")) as {
    packages: Record<string, { dev?: boolean; license?: string }>;
  };
  assert.deepEqual(licenseProblems(lock, BUILD_TOOL_EXCEPTIONS), []);
  const shipped = Object.entries(lock.packages).filter(([name, entry]) => name !== "" && entry.dev !== true);
  assert.ok(shipped.some(([name]) => name === "node_modules/electron-updater"), "electron-updater ships");
  assert.equal(lock.packages["node_modules/electron-builder"]?.dev, true, "electron-builder is a build tool");
  assert.deepEqual(licenseProblems({ packages: Object.fromEntries(shipped) }, {}), [], "nothing shipped needs an exception");
});
