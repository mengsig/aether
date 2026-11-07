// @vitest-environment happy-dom
import { AppProvider, createApp, registerRoutes, type App } from "@aether-ui/core";
import type { DesktopBridge, PreferenceKey } from "@aether-ui/protocol";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, test, vi } from "vitest";
import { bridgeMessage, checkHostAddress } from "./desktop-connect.tsx";
import PairScreen from "./pair-screen.tsx";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const storage = { get: async () => undefined, set: async () => {} };
let root: Root | null = null;

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  document.body.innerHTML = "";
});

interface FakeBridge {
  readonly bridge: DesktopBridge;
  readonly prefs: Map<PreferenceKey, unknown>;
  readonly openRemote: ReturnType<typeof vi.fn>;
}

function fakeBridge(origins: string[] = []): FakeBridge {
  const prefs = new Map<PreferenceKey, unknown>([["remote.origins", origins]]);
  const openRemote = vi.fn<(origin: string) => Promise<void>>(async () => {});
  const bridge = {
    version: 1,
    getPreference: async (key: PreferenceKey) => prefs.get(key) ?? null,
    setPreference: async (key: PreferenceKey, value: unknown) => { prefs.set(key, value); },
    openRemote,
  } as unknown as DesktopBridge;
  return { bridge, prefs, openRemote };
}

async function render(app: App, daemon = "aether"): Promise<HTMLElement> {
  const container = document.body.appendChild(document.createElement("main"));
  root = createRoot(container);
  const mounted = root;
  await act(async () => mounted.render(<AppProvider app={app}><PairScreen params={{ daemon }} /></AppProvider>));
  return container;
}

const radio = (scope: ParentNode, name: string) => {
  const found = [...scope.querySelectorAll<HTMLInputElement>("input[type=radio]")].find((input) => input.getAttribute("aria-label") === name);
  if (!found) throw new Error(`no radio named ${name}`);
  return found;
};

const button = (scope: ParentNode, name: string) => {
  const found = [...scope.querySelectorAll("button")].find((b) => b.textContent?.trim() === name);
  if (!found) throw new Error(`no button named ${name}`);
  return found;
};

function field(scope: ParentNode, label: string): HTMLInputElement {
  const labelled = [...scope.querySelectorAll("label")].find((l) => l.textContent?.trim() === label);
  const input = labelled ? scope.querySelector(`[id="${labelled.htmlFor}"]`) : null;
  if (!(input instanceof HTMLInputElement)) throw new Error(`no field labelled ${label}`);
  return input;
}

async function type(input: HTMLInputElement, value: string): Promise<void> {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  await act(async () => {
    setter?.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

const settle = () => act(async () => { for (let turn = 0; turn < 10; turn += 1) await Promise.resolve(); });

async function press(target: HTMLElement): Promise<void> {
  await act(async () => target.click());
  await settle();
}

async function openAnother(fake: FakeBridge): Promise<HTMLElement> {
  const view = await render(createApp({ kind: "electron", desktop: fake.bridge, storage }, { endpoints: [] }));
  await press(radio(view, "Another computer"));
  return view;
}

describe("host addresses", () => {
  test("a bare host gets https:, and only an origin passes", () => {
    expect(checkHostAddress(" Host.Example.com:8443 ")).toEqual({ origin: "https://host.example.com:8443" });
    expect(checkHostAddress("https://host.example.com:443/")).toEqual({ origin: "https://host.example.com" });
    expect(checkHostAddress("http://host.example.com")).toMatchObject({ error: expect.stringContaining("https: only") });
    expect(checkHostAddress("https://me:pw@host.example.com")).toMatchObject({ error: expect.stringContaining("user name") });
    expect(checkHostAddress("host.example.com/pair")).toMatchObject({ error: expect.stringContaining("without a path") });
    expect(checkHostAddress("https://host.example.com/?a=1")).toMatchObject({ error: expect.stringContaining("without a path") });
    expect(checkHostAddress("")).toMatchObject({ error: expect.stringContaining("Type the host's address") });
  });

  test("a bridge refusal loses Electron's wrapping", () => {
    expect(bridgeMessage(new Error("Error invoking remote method 'desktop:openRemote': BridgeRefusal: https://h.example is not in remote.origins")))
      .toBe("https://h.example is not in remote.origins");
    expect(bridgeMessage(new Error(""))).toBe("The desktop app refused without giving a reason.");
  });
});

describe("the desktop connect screen", () => {
  test("offers this computer and another computer, and asks for no code", async () => {
    const view = await render(createApp({ kind: "electron", desktop: fakeBridge().bridge, storage }, { endpoints: [] }));
    expect(view.querySelector("h2")?.textContent).toBe("Where does Aether run?");
    expect(radio(view, "This computer").checked).toBe(false);
    expect(radio(view, "Another computer").checked).toBe(false);
    expect([...view.querySelectorAll("label")].some((l) => l.textContent?.trim() === "Pairing code")).toBe(false);
  });

  test("this computer links to Connections until a setup route is registered", async () => {
    const app = createApp({ kind: "electron", desktop: fakeBridge().bridge, storage }, { endpoints: [] });
    const view = await render(app);
    await press(radio(view, "This computer"));
    expect(view.querySelector("a.app-pair-link")?.getAttribute("href")).toBe("#/settings/connections");
    await act(async () => registerRoutes([{ path: "#/setup", owner: "test", load: async () => ({ default: () => null }) }]));
    const link = view.querySelector("a.app-pair-link");
    expect(link?.getAttribute("href")).toBe("#/setup");
    expect(link?.textContent).toBe("Set up Aether on this computer");
  });

  test("an invalid address shows an inline error and sends nothing", async () => {
    const fake = fakeBridge();
    const setPreference = vi.spyOn(fake.bridge, "setPreference");
    const view = await openAnother(fake);
    const input = field(view, "Host address");
    await type(input, "http://host.example.com");
    await press(button(view, "Open the host"));
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(view.querySelector(".ui-field-error")?.textContent).toContain("https: only");
    expect(setPreference).not.toHaveBeenCalled();
    expect(fake.openRemote).not.toHaveBeenCalled();
  });

  test("a valid address is added to remote.origins once and opened in its own window", async () => {
    const fake = fakeBridge(["https://other.example"]);
    const view = await openAnother(fake);
    await type(field(view, "Host address"), "host.example.com");
    await press(button(view, "Open the host"));
    expect(fake.prefs.get("remote.origins")).toEqual(["https://other.example", "https://host.example.com"]);
    expect(fake.openRemote).toHaveBeenCalledWith("https://host.example.com");
    expect(view.querySelector("[role=status]")?.textContent).toContain("Opened https://host.example.com in its own window. Pair there with the code the host shows.");
    await press(button(view, "Open the host"));
    expect(fake.prefs.get("remote.origins")).toEqual(["https://other.example", "https://host.example.com"]);
    expect(fake.openRemote).toHaveBeenCalledTimes(2);
  });

  test("a refused open shows the bridge's message, and Retry tries again", async () => {
    const fake = fakeBridge();
    fake.openRemote.mockRejectedValueOnce(new Error("Error invoking remote method 'desktop:openRemote': BridgeRefusal: the window could not open"));
    const view = await openAnother(fake);
    await type(field(view, "Host address"), "https://host.example.com");
    await press(button(view, "Open the host"));
    const alert = view.querySelector("[role=alert]");
    expect(alert?.textContent).toContain("Could not open https://host.example.com");
    expect(alert?.textContent).toContain("the window could not open");
    await press(button(view, "Retry"));
    expect(fake.openRemote).toHaveBeenCalledTimes(2);
    expect(view.querySelector("[role=alert]")).toBeNull();
    expect(view.querySelector("[role=status]")?.textContent).toContain("Opened https://host.example.com");
  });
});

describe("the browser pairing form", () => {
  test("a host with no pairing service fails with a plain sentence, and Start again returns to the form", async () => {
    const fetcher = vi.fn(async () => new Response(null, { status: 404 }));
    const view = await render(createApp({ kind: "browser", storage }, { endpoints: [], fetch: fetcher as unknown as typeof fetch }), "archon");
    await type(field(view, "Pairing code"), "ABCD-1234");
    await press(button(view, "Pair"));
    const alert = view.querySelector("[role=alert]");
    expect(alert?.textContent).toContain("The host did not answer as a pairing service (HTTP 404). Check the address.");
    expect(alert?.textContent).not.toContain("JSON");
    await press(button(view, "Start again"));
    expect(field(view, "Pairing code")).toBeInstanceOf(HTMLInputElement);
  });
});
