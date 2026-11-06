// @vitest-environment happy-dom
// The Noesis settings sheet's converted fields: model pickers that hide API-billed models unless they
// are allowed, a Default option that sends what an empty field sent, budgets checked while typing, and
// the off state's "Turn on Noesis" opening the sheet with the switch already on.

import { drafts } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { afterEach, describe, expect, test, vi } from "vitest";
import { SETTLE_MS } from "./data.ts";
import { $, byText, click, CONN, connected, hashChanged, type, type Harness } from "./fixture/harness.test-util.tsx";
import { AUTH, MODELS, scenario, SETTINGS, statusOf } from "./fixture/scenario.ts";
import { modelList } from "./models.ts";
import { draftOf, patchOf, settingsDraftKey } from "./settings-draft.ts";

const SETTINGS_DRAFT_KEY = settingsDraftKey(CONN);
import { DEFAULT_MODEL_LABEL, hiddenModelNote } from "./SettingsSheet.tsx";
import SessionsRoute from "./SessionsScreen.tsx";

let harness: Harness | null = null;
afterEach(() => {
  harness?.unmount();
  harness = null;
});

function withBilling(allowApi: boolean, enabled = true): aether.NoesisStatus {
  const status = statusOf(enabled);
  const settings = { ...status.settings, billing: { allow_api: allowApi } };
  return { ...status, settings, effective: settings };
}

async function openSheet(status: aether.NoesisStatus, transport: "ipc" | "ws-aether" = "ipc"): Promise<{ h: Harness; sheet: HTMLElement }> {
  const h = await connected(scenario(status.enabled, { transport, status }), vi);
  harness = h;
  location.hash = "#/learn?panel=settings";
  await h.mount(<SessionsRoute />);
  await hashChanged(h);
  await h.settle(SETTLE_MS);
  return { h, sheet: $(document, "dialog[open]") };
}

async function optionsOf(h: Harness, sheet: HTMLElement, testid: string): Promise<string[]> {
  await click(h, $(sheet, `[data-testid=${testid}]`));
  const options = [...sheet.querySelectorAll("[role=option]")].map((option) => option.querySelector(".ui-picker-option-label")?.textContent ?? "");
  await click(h, $(sheet, `[data-testid=${testid}]`));
  return options;
}

const configures = (h: Harness) => h.daemon.sent.filter((c) => c.cmd === "noesis_configure").map((c) => c.patch);

describe("model pickers", () => {
  test("API-billed models are hidden while they are not allowed, and listed once the switch is on", async () => {
    const { h, sheet } = await openSheet(withBilling(false));
    const allow = $(sheet, "[data-testid=noesis-allow-api]");
    expect(allow.getAttribute("aria-checked")).toBe("false");
    expect(allow.textContent).toContain("Noesis spends your subscription by default");
    expect(await optionsOf(h, sheet, "noesis-model-strong")).toEqual([DEFAULT_MODEL_LABEL, "claude-sonnet-5-5", "gpt-6-luna"]);
    await click(h, allow);
    expect(allow.getAttribute("aria-checked")).toBe("true");
    expect(await optionsOf(h, sheet, "noesis-model-cheap")).toEqual([DEFAULT_MODEL_LABEL, "claude-opus-5-5", "claude-haiku-5-5", "claude-sonnet-5-5", "gpt-6-luna"]);
  });

  test("a saved API-billed model that is hidden says why under its picker", async () => {
    const { sheet } = await openSheet(withBilling(false));
    const strong = $(sheet, "[data-testid=noesis-model-strong]").closest(".ui-picker");
    expect(strong?.textContent).toContain("anthropic/claude-opus-5-5 is not offered");
    expect(hiddenModelNote("", { models: MODELS, auth: AUTH, loaded: true, error: null }, false)).toBeUndefined();
    expect(hiddenModelNote("codex/gpt-6-luna", { models: MODELS, auth: AUTH, loaded: true, error: null }, false)).toBeUndefined();
  });

  test("Default sends exactly what an empty field sent: the model cleared to null", async () => {
    const { h, sheet } = await openSheet(withBilling(true));
    await click(h, $(sheet, "[data-testid=noesis-model-strong]"));
    await click(h, byText(sheet, "[role=option]", DEFAULT_MODEL_LABEL));
    await click(h, $(sheet, "[data-testid=noesis-save]"));
    expect(configures(h)).toEqual([{ models: { strong: null } }]);
    const emptyField = patchOf(SETTINGS, { ...draftOf(SETTINGS), strong: "" }).patch;
    const typedSpaces = patchOf(SETTINGS, { ...draftOf(SETTINGS), strong: "   " }).patch;
    expect(emptyField).toEqual({ models: { strong: null } });
    expect(typedSpaces).toEqual(emptyField);
  });

  test("the model list is checked at the boundary", () => {
    expect(modelList(undefined)).toEqual([]);
    expect(modelList(MODELS)).toHaveLength(MODELS.length);
    expect(() => modelList({ models: MODELS })).toThrow(/answers with a list/);
    expect(() => modelList([{ model: { provider: "", model: "x" }, configured: true }])).toThrow(/provider-qualified/);
  });
});

describe("budgets", () => {
  test("are checked while typing: the error sits under the field and Save stays off until it is fixed", async () => {
    const { h, sheet } = await openSheet(withBilling(true));
    const save = $(sheet, "[data-testid=noesis-save]") as HTMLButtonElement;
    const teaching = $(sheet, "[data-testid=noesis-budget-teaching]");
    await type(h, teaching, "-1");
    expect(teaching.getAttribute("aria-invalid")).toBe("true");
    expect(teaching.closest(".ui-field")?.querySelector(".ui-field-error")?.textContent).toBe("Enter an amount of 0 or more");
    expect(save.disabled).toBe(true);
    await type(h, $(sheet, "[data-testid=noesis-budget-tokens]"), "1.5");
    expect(sheet.textContent).toContain("Enter a whole number of 0 or more");
    await type(h, teaching, "3.5");
    await type(h, $(sheet, "[data-testid=noesis-budget-tokens]"), "1000");
    expect(teaching.getAttribute("aria-invalid")).toBeNull();
    expect(save.disabled).toBe(false);
    await click(h, save);
    expect(h.daemon.privileged).toEqual([{ kind: "noesis.configure", conn: "local:faux-main", patch: { "noesis.budget.interactive_usd_daily": 3.5, "noesis.budget.background_tokens_daily": 1000 } }]);
    expect(drafts.get(SETTINGS_DRAFT_KEY)).toBeUndefined();
  });

  test("an unsaved edit is kept as a draft across closing the sheet, and can be discarded", async () => {
    const { h, sheet } = await openSheet(withBilling(true));
    await type(h, $(sheet, "[data-testid=noesis-budget-background]"), "4");
    expect(drafts.get(SETTINGS_DRAFT_KEY)).toMatchObject({ backgroundUsd: "4" });
    await h.remount(<SessionsRoute />);
    await h.settle(SETTLE_MS);
    const again = $(document, "dialog[open]");
    expect(($(again, "[data-testid=noesis-budget-background]") as HTMLInputElement).value).toBe("4");
    await click(h, byText(again, "button", "Discard changes"));
    expect(($(again, "[data-testid=noesis-budget-background]") as HTMLInputElement).value).toBe("2");
    expect(drafts.get(SETTINGS_DRAFT_KEY)).toBeUndefined();
  });
});

describe("turning Noesis on", () => {
  test("the off state's primary action opens the sheet with the switch on, and saving sends only the switch", async () => {
    const h = await connected(scenario(false, { transport: "ipc" }), vi);
    harness = h;
    location.hash = "#/learn";
    const root = await h.mount(<SessionsRoute />);
    const turnOn = $(root, "[data-testid=noesis-turn-on]");
    expect(turnOn.textContent).toContain("subscription models");
    expect(turnOn.textContent).toContain("daily budgets");
    const before = h.daemon.sent.length;
    await click(h, byText(turnOn, "button", "Turn on Noesis"));
    await hashChanged(h);
    await h.settle(SETTLE_MS);
    expect(location.hash).toBe("#/learn?panel=settings");
    const sheet = $(document, "dialog[open]");
    expect($(sheet, "[data-testid=noesis-enable]").getAttribute("aria-checked")).toBe("true");
    expect(h.daemon.names().slice(before).filter((cmd) => cmd !== "noesis_status").sort()).toEqual(["auth_status", "list_models"]);
    await click(h, $(sheet, "[data-testid=noesis-save]"));
    expect(h.daemon.privileged).toEqual([{ kind: "noesis.configure", conn: "local:faux-main", patch: { "noesis.enabled": true } }]);
    expect(drafts.get(SETTINGS_DRAFT_KEY)).toBeUndefined();
  });

  test("a device that is not the host is told who can turn it on, and opens the settings to look", async () => {
    const h = await connected(scenario(false), vi);
    harness = h;
    const root = await h.mount(<SessionsRoute />);
    expect(root.textContent).toContain("Only the daemon's own host can turn it on");
    expect(root.querySelector("[data-testid=noesis-turn-on-action]")).toBeNull();
    expect(byText(root, "button", "Open Noesis settings")).toBeTruthy();
  });

  test("a refusal for want of a model names both next steps: Noesis settings and signing in to a provider", async () => {
    const h = await connected(scenario(true, { refuse: "no ready model for Noesis" }), vi);
    harness = h;
    location.hash = "#/learn";
    const root = await h.mount(<SessionsRoute />);
    await type(h, $(root, "[data-testid=learn-goal]"), "Anything");
    await click(h, $(root, "[data-testid=learn-start]"));
    const links = [...root.querySelectorAll(".learn-callout a")].map((a) => a.getAttribute("href"));
    expect(links).toEqual(["#/learn?panel=settings", "#/settings/providers"]);
  });
});
