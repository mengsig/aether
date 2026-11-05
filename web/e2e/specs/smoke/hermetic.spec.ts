// The hermetic guard: the daemons run with every home, XDG, Aether and Archon directory inside
// the stack root and no credential; HOME stays empty; every socket the audit saw is loopback.

import { readdir, readFile, stat } from "node:fs/promises";
import path from "node:path";
import { checkEnvironment, decodeAddress, isLoopback } from "../../harness/audit.ts";
import { expect, test } from "../../harness/fixtures.ts";

test("the audit classifies /proc addresses correctly", () => {
  expect(decodeAddress("0100007F:1F90")).toBe("127.0.0.1:8080");
  expect(decodeAddress("010200C0:0009")).toBe("192.0.2.1:9");
  expect(isLoopback(decodeAddress("0100007F:1F90"))).toBe(true);
  expect(isLoopback(decodeAddress("010200C0:0009"))).toBe(false);
  expect(isLoopback(decodeAddress("00000000:0050"))).toBe(false);
  expect(isLoopback(decodeAddress("00000000000000000000000001000000:0050"))).toBe(true);
  expect(isLoopback(decodeAddress("0000000000000000FFFF00000100007F:0050"))).toBe(true);
  expect(isLoopback(decodeAddress("B80D0120000000000000000001000000:0050"))).toBe(false);
});

test("the daemons are confined to the stack root and loopback", async ({ stack, audit }) => {
  const { root, home } = stack.layout;
  for (const pid of [stack.aether.pid, stack.archon.pid]) {
    expect(pid, "both daemons run").toBeDefined();
    expect(await checkEnvironment(pid as number, root)).toEqual([]);
  }
  expect(await readdir(home), "HOME stays empty").toEqual([]);
  expect((await stat(home)).mode & 0o777, "HOME is read-only").toBe(0o500);

  for (const file of [path.join(stack.layout.aether.config, "config.toml"), path.join(stack.layout.aether.config, "providers.toml"),
    path.join(stack.layout.archon.config, "config.toml")]) {
    const hosts = [...(await readFile(file, "utf8")).matchAll(/(?:https?:\/\/|address = ")([^:/"\]]+)/g)].map((match) => match[1]);
    expect(hosts.length, `${file} names an address`).toBeGreaterThan(0);
    expect(new Set(hosts), `${file} names loopback only`).toEqual(new Set(["127.0.0.1"]));
  }

  await expect.poll(() => audit.samples, { message: "the audit is sampling" }).toBeGreaterThan(5);
  expect(audit.socketsSeen, "the audit sees the daemons' inet sockets").toBeGreaterThanOrEqual(3);
  expect(audit.violations).toEqual([]);
});
