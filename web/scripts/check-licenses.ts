// Fails when a package in web/package-lock.json breaks the licence rule of scripts/licenses.ts.

import { readFileSync } from "node:fs";
import path from "node:path";
import { licenseProblems } from "./licenses.ts";

const lockFile = path.resolve(import.meta.dirname, "..", "package-lock.json");
const lock: unknown = JSON.parse(readFileSync(lockFile, "utf8"));
const problems = licenseProblems(lock);
for (const problem of problems) console.error(problem);
const count = Object.keys((lock as { packages: object }).packages).length - 1;
if (problems.length > 0) {
  console.error(`${problems.length} licence problem(s) in ${lockFile}`);
  process.exitCode = 1;
} else {
  console.log(`licences: ${count} locked packages are MIT, ISC, BSD or Apache-2.0, or a listed dev-only exception`);
}
