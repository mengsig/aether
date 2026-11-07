// @vitest-environment happy-dom
// Providers: the stored provider, auth and MCP replies, the refreshes (operate) and the engine
// sign-in (host only, through the desktop); no credential is typed in or shown.

import { afterEach, describe, expect, test } from "vitest";
import { connected, click, buttonNamed, type Harness } from "../kit/fixture/harness.test-util.tsx";
import { fakeDesktop } from "../kit/fixture/desktop.test-util.ts";
import { ProvidersPage } from "./ProvidersPage.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

const health = { provider: "claude-code", running: 1 };
const claude = {
  provider: "claude-code", name: "Claude Code", configured: true, enabled: true, engine: true, models: 3, protocol: "engine", open_model_list: false,
  health, auth_source: "subscription", headroom: { stale: false, windows: [{ name: "5h", remaining_percent: 62.4 }] },
};
const local = { provider: "local", name: "Local models", configured: false, enabled: false, engine: false, models: 0, protocol: "openai", open_model_list: true, health: { provider: "local", running: 0 } };
const auth = [{ provider: "claude-code", provider_name: "Claude Code", configured: true, source: "keychain", subscription: true, detail: "token sk-live-should-not-show" }];
const mcp = [{ name: "github", state: "ready", transport: "stdio", tools: [{ name: "a" }, { name: "b" }], checked_at: "2026-11-04T08:00:00Z" }];

const script = {
  providers: () => ({ tag: "providers", data: [claude, local] }),
  auth_status: () => ({ tag: "auth", data: auth }),
  mcp_servers: () => ({ tag: "mcp_servers", data: mcp }),
  refresh_provider_usage: () => ({ tag: "provider_usage", data: null }),
};

describe("ProvidersPage", () => {
  test("shows each provider, its auth source, headroom, health and the MCP servers from the store", async () => {
    const h = current = await connected({ role: "operator", script });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    const row = root.querySelector('li[aria-label="Claude Code"]');
    expect(row?.textContent).toContain("Ready, 1 running");
    expect(row?.textContent).toContain("5h 62% left");
    expect(row?.textContent).toContain("subscription");
    expect(row?.textContent).not.toContain("Healthy");
    expect(root.querySelector('li[aria-label="Local models"]')?.textContent).toContain("Turned off");
    expect(root.querySelector('li[aria-label="github"]')?.textContent).toContain("2 tools");
    expect(h.app.store.get().reads[h.conn]?.["settings.auth"]?.value).toEqual(auth);
    expect(root.textContent).not.toContain("sk-live-should-not-show");
  });

  test("refreshing usage sends refresh_provider_usage and rereads the providers", async () => {
    const h = current = await connected({ role: "operator", script });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    await click(h, root, "Refresh usage");
    expect(h.daemon.commands("refresh_provider_usage")).toHaveLength(1);
    expect(h.daemon.commands("providers")).toHaveLength(2);
    expect(root.textContent).toContain("Usage refreshed");
  });

  test("checking MCP servers again rereads with refresh: true into the same store key", async () => {
    const h = current = await connected({ role: "operator", script });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    expect(h.daemon.commands("mcp_servers")[0]).not.toHaveProperty("refresh");
    await click(h, root, "Check servers again");
    expect(h.daemon.commands("mcp_servers").at(-1)).toMatchObject({ refresh: true });
    expect(h.app.store.get().reads[h.conn]?.["settings.mcp"]?.value).toEqual(mcp);
  });

  test("an observer still reads, but every control is disabled with its reason", async () => {
    const h = current = await connected({ role: "observer", script });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    expect(root.textContent).toContain("Claude Code");
    for (const label of ["Refresh usage", "Check servers again", "Sign in again"]) expect(buttonNamed(root, label).disabled).toBe(true);
    const reasons = [...root.querySelectorAll(".st-reason")].map((r) => r.textContent);
    expect(reasons).toContain("Observer devices cannot change daemon state");
    expect(reasons).toContain("Only the daemon's own host can do this");
  });

  test("sign-in is host only: marked and refused to a remote operator", async () => {
    const h = current = await connected({ role: "operator", script });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    expect(buttonNamed(root, "Sign in again").disabled).toBe(true);
    expect(root.textContent).toContain("Host only");
    expect(root.querySelectorAll('li[aria-label="Local models"] button')).toHaveLength(0);
  });

  test("on the host, sign-in asks first and opens the engine login through the desktop", async () => {
    const desktop = fakeDesktop({ url: "https://auth.example/device?code=SECRET-CODE" });
    const h = current = await connected({ role: "local", desktop: desktop.bridge, script });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    await click(h, root, "Sign in again");
    expect(desktop.actions).toHaveLength(0);
    expect(document.body.textContent).not.toMatch(/terminal|on the host/);
    await click(h, document.body, "Continue");
    expect(desktop.actions).toEqual([{ kind: "engine.login", engine: "claude", conn: "bridge-1" }]);
    expect(h.daemon.commands("auth_status")).toHaveLength(2);
    expect(document.body.textContent).toContain("A sign-in window for Claude Code opened. Finish there, then check again.");
    expect(document.body.textContent).not.toContain("SECRET-CODE");
    expect(root.querySelector("input")).toBeNull();
  });

  test("a lost reply on refreshing usage, which is unkeyed, is noted in the recovery review and not retried", async () => {
    const h = current = await connected({ role: "operator", script: { ...script, refresh_provider_usage: () => "hold" } });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    await click(h, root, "Refresh usage");
    h.daemon.drop();
    await h.settle();
    expect(root.querySelector(".st-note[data-state]")?.getAttribute("data-state")).toBe("unconfirmed");
    expect(root.textContent).toContain("Open the recovery review");
    expect(h.app.store.get().aether[h.conn]?.outbox).toEqual([expect.objectContaining({ unkeyed: true, state: "unknown", command: expect.objectContaining({ cmd: "refresh_provider_usage" }) })]);
    expect(h.daemon.commands("refresh_provider_usage")).toHaveLength(1);
  });

  test("UX-003: a provider that cannot run work says why and offers the fix, never a healthy badge", async () => {
    const signedOut = { ...claude, configured: false, auth_source: null, headroom: undefined };
    const noKey = { ...local, enabled: true };
    const lapsed = [{ ...auth[0], configured: false, source: null }];
    const h = current = await connected({ role: "local", desktop: fakeDesktop().bridge, script: {
      ...script, providers: () => ({ tag: "providers", data: [signedOut, noKey] }), auth_status: () => ({ tag: "auth", data: lapsed }),
    } });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    const row = root.querySelector('li[aria-label="Claude Code"]');
    expect(row?.querySelector(".ui-badge")?.textContent).toBe("Not signed in");
    expect(row?.textContent).toContain("Sign in to Claude Code to use its models.");
    expect(buttonNamed(row as Element, "Sign in to Claude Code").getAttribute("data-variant")).toBe("primary");
    const keyless = root.querySelector('li[aria-label="Local models"]');
    expect(keyless?.textContent).toContain("Not set up");
    expect(root.textContent).not.toMatch(/Healthy|Configured|Enabled/);
  });

  test("UX-043: on this computer, missing Archon tools offer Connect Archon tools, which runs the setup steps", async () => {
    const steps: string[] = [];
    const desktop = fakeDesktop();
    const bridge = Object.assign(desktop.bridge, { setup: async (step: string) => { steps.push(step); throw new Error("Archon could not be reached. Is it installed?"); } });
    const h = current = await connected({ role: "local", desktop: bridge, script });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    expect(root.textContent).toContain("Archon's tools are not connected to Aether yet.");
    await click(h, root, "Connect Archon tools");
    expect(steps).toEqual(["connect"]);
    expect(root.querySelector('.su-step[data-state="failed"]')?.textContent).toContain("Archon could not be reached. Is it installed?");
    await click(h, root, "Try again");
    expect(steps).toEqual(["connect", "connect"]);
  });

  test("UX-043: a ready Archon server needs no repair, and a remote device is told where to fix it", async () => {
    const ready = [...mcp, { name: "archon", state: "ready", transport: "stdio", tools: [{ name: "archon_status" }] }];
    const fine = current = await connected({ role: "local", desktop: fakeDesktop().bridge, script: { ...script, mcp_servers: () => ({ tag: "mcp_servers", data: ready }) } });
    const okRoot = await fine.mount(<ProvidersPage conn={fine.conn} />);
    expect(okRoot.querySelector('li[aria-label="archon"]')?.textContent).toContain("Ready");
    expect(okRoot.textContent).not.toContain("Connect Archon tools");
    fine.unmount();
    const remote = current = await connected({ role: "operator", script });
    const root = await remote.mount(<ProvidersPage conn={remote.conn} />);
    expect(root.textContent).toContain("Connect them from the desktop app on the computer running Aether.");
    expect(root.textContent).not.toContain("Connect Archon tools");
  });

  test("an empty provider list says so", async () => {
    const h = current = await connected({ role: "operator", script: { ...script, providers: () => ({ tag: "providers", data: [] }) } });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    expect(root.textContent).toContain("No providers");
  });
});
