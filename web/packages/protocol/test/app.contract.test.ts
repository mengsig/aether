import { readFileSync } from "node:fs";
import path from "node:path";
import ts from "typescript";
import { describe, expect, it } from "vitest";

const CONTRACT = path.join(import.meta.dirname, "fixtures", "contracts-app-types.ts.txt");
const APP = path.join(import.meta.dirname, "..", "src", "app.ts");
// Declared in app.ts because §2.4 names them without declaring them.
const UNDECLARED_IN_CONTRACT = ["LocalDaemon", "NoesisSettingsPatch"];

interface Declaration {
  readonly text: string;
  readonly exported: boolean;
}

/** Each top-level declaration, printed without comments or `export` so layout cannot matter. */
function declarations(fileName: string, text: string): Map<string, Declaration> {
  const source = ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, false, ts.ScriptKind.TS);
  const printer = ts.createPrinter({ removeComments: true });
  const found = new Map<string, Declaration>();
  for (const statement of source.statements) {
    if (!ts.isInterfaceDeclaration(statement) && !ts.isTypeAliasDeclaration(statement)) {
      throw new Error(`${fileName}: only interfaces and type aliases belong here, not ${ts.SyntaxKind[statement.kind]}`);
    }
    const name = statement.name.text;
    if (found.has(name)) throw new Error(`${fileName}: ${name} is declared twice`);
    const exported = ts.getModifiers(statement)?.some((m) => m.kind === ts.SyntaxKind.ExportKeyword) ?? false;
    const printed = printer.printNode(ts.EmitHint.Unspecified, statement, source).replace(/^export /, "");
    found.set(name, { text: printed, exported });
  }
  return found;
}

/** The names whose declarations differ between the contract and an app.ts text. */
function mismatches(appText: string): string[] {
  const contract = declarations(CONTRACT, readFileSync(CONTRACT, "utf8"));
  const app = declarations(APP, appText);
  return [...contract].filter(([name, wanted]) => app.get(name)?.text !== wanted.text).map(([name]) => name);
}

describe("app.ts against CONTRACTS.md §2.4 and §2.5", () => {
  const appText = readFileSync(APP, "utf8");
  const contract = declarations(CONTRACT, readFileSync(CONTRACT, "utf8"));
  const app = declarations(APP, appText);

  it("reads the whole contract", () => {
    // Three declarations in §2.4 and eighteen in §2.5.
    expect(contract.size).toBe(21);
    for (const name of ["DesktopBridge", "PrivilegedAction", "HostAdapter", "RouteDef", "GraphModel", "Layout"]) {
      expect(contract.has(name), name).toBe(true);
    }
  });

  it("declares every contract type unchanged", () => {
    expect(mismatches(appText)).toEqual([]);
  });

  it("declares nothing the contract does not name, and exports everything", () => {
    const extra = [...app.keys()].filter((name) => !contract.has(name)).sort();
    expect(extra).toEqual([...UNDECLARED_IN_CONTRACT].sort());
    expect([...app].filter(([, declaration]) => !declaration.exported).map(([name]) => name)).toEqual([]);
  });

  it("notices a changed member", () => {
    const changed = appText.replace('"connecting" | "pairing"', '"connecting"');
    expect(changed).not.toBe(appText);
    expect(mismatches(changed)).toEqual(["ConnState"]);
  });
});
