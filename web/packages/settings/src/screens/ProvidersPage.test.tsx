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
    expect(row?.textContent).toContain("Configured");
    expect(row?.textContent).toContain("5h 62% left");
    expect(row?.textContent).toContain("subscription");
    expect(row?.textContent).toContain("Healthy, 1 running");
    expect(root.querySelector('li[aria-label="Local models"]')?.textContent).toContain("Not configured");
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
    for (const label of ["Refresh usage", "Check servers again", "Sign in"]) expect(buttonNamed(root, label).disabled).toBe(true);
    const reasons = [...root.querySelectorAll(".st-reason")].map((r) => r.textContent);
    expect(reasons).toContain("Observer devices cannot change daemon state");
    expect(reasons).toContain("Only the daemon's own host can do this");
  });

  test("sign-in is host only: marked and refused to a remote operator", async () => {
    const h = current = await connected({ role: "operator", script });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    expect(buttonNamed(root, "Sign in").disabled).toBe(true);
    expect(root.textContent).toContain("Host only");
    expect(root.querySelectorAll('li[aria-label="Local models"] button')).toHaveLength(0);
  });

  test("on the host, sign-in asks first and opens the engine login through the desktop", async () => {
    const desktop = fakeDesktop({ url: "https://auth.example/device?code=SECRET-CODE" });
    const h = current = await connected({ role: "local", desktop: desktop.bridge, script });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    await click(h, root, "Sign in");
    expect(desktop.actions).toHaveLength(0);
    await click(h, document.body, "Open terminal");
    expect(desktop.actions).toEqual([{ kind: "engine.login", engine: "claude", conn: "bridge-1" }]);
    expect(h.daemon.commands("auth_status")).toHaveLength(2);
    expect(document.body.textContent).toContain("Sign-in for Claude Code opened in a terminal on the host");
    expect(document.body.textContent).not.toContain("SECRET-CODE");
    expect(root.querySelector("input")).toBeNull();
  });

  test("a lost reply on refreshing usage, which is unkeyed, is reported unknown without the recovery review", async () => {
    const h = current = await connected({ role: "operator", script: { ...script, refresh_provider_usage: () => "hold" } });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    await click(h, root, "Refresh usage");
    h.daemon.drop();
    await h.settle();
    expect(root.querySelector(".st-note")?.textContent).toContain("Check its current state before trying again");
    expect(root.textContent).not.toContain("Open the recovery review");
    expect(h.app.store.get().aether[h.conn]?.outbox ?? []).toEqual([]);
    expect(h.daemon.commands("refresh_provider_usage")).toHaveLength(1);
  });

  test("an empty provider list says so", async () => {
    const h = current = await connected({ role: "operator", script: { ...script, providers: () => ({ tag: "providers", data: [] }) } });
    const root = await h.mount(<ProvidersPage conn={h.conn} />);
    expect(root.textContent).toContain("No providers");
  });
});
