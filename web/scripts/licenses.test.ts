import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { licenseAllowed, licenseProblems } from "./licenses.ts";

const exceptions = { minimatch: { license: "BlueOak-1.0.0", reason: "test" } };

function lock(packages: Record<string, object>) {
  return { lockfileVersion: 3, packages: { "": { name: "aether-web" }, ...packages } };
}

describe("licence rule", () => {
  it("holds for the committed lockfile", () => {
    const committed: unknown = JSON.parse(readFileSync(path.join(import.meta.dirname, "..", "package-lock.json"), "utf8"));
    expect(licenseProblems(committed)).toEqual([]);
  });

  it("accepts MIT, ISC, BSD and Apache-2.0, and a choice that includes one", () => {
    for (const license of ["MIT", "ISC", "BSD-2-Clause", "BSD-3-Clause", "Apache-2.0", "(MIT OR GPL-3.0-only)"]) {
      expect(licenseAllowed(license), license).toBe(true);
    }
    for (const license of ["GPL-3.0-only", "MPL-2.0", "(MIT AND GPL-3.0-only)", "(MIT OR (GPL-2.0 AND ISC))", "UNLICENSED"]) {
      expect(licenseAllowed(license), license).toBe(false);
    }
  });

  it("refuses a copyleft or missing licence", () => {
    const problems = licenseProblems(lock({
      "node_modules/react": { license: "MIT" },
      "node_modules/lightningcss": { license: "MPL-2.0", dev: true },
      "node_modules/mystery": { dev: true },
      "node_modules/minimatch": { license: "BlueOak-1.0.0", dev: true },
    }), exceptions);
    expect(problems).toEqual([
      "node_modules/lightningcss: MPL-2.0 is not allowed",
      "node_modules/mystery: no licence in the lockfile",
    ]);
  });

  it("allows an exception only in dev-only tools, and only while it is needed", () => {
    expect(licenseProblems(lock({ "node_modules/a/node_modules/minimatch": { license: "BlueOak-1.0.0" } }), exceptions))
      .toEqual(["node_modules/a/node_modules/minimatch: BlueOak-1.0.0 is allowed in dev-only tools, not at runtime",
        "exception for minimatch is unused: remove it from scripts/licenses.ts"]);
    expect(licenseProblems(lock({ "node_modules/react": { license: "MIT" } }), exceptions))
      .toEqual(["exception for minimatch is unused: remove it from scripts/licenses.ts"]);
    expect(licenseProblems({ lockfileVersion: 1 })).toEqual(["package-lock.json has no packages map (lockfile v2 or later)"]);
  });
});
