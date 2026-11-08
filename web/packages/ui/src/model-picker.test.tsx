// @vitest-environment happy-dom
import { describe, expect, test } from "vitest";
import type { aether } from "@aether-ui/protocol";
import { billingOf, ModelPicker, modelOptions } from "./model-picker.tsx";
import { click, render } from "./render.test-util.tsx";

const capabilities = { context_window: 200000, max_output_tokens: 8192, tools: true } as unknown as aether.CapabilityProfile;
const model = (provider: string, id: string, extra: Partial<aether.ModelInfo> = {}): aether.ModelInfo => ({
  model: { provider, model: id }, name: id, provider_name: provider === "claude-code" ? "Claude Code" : provider === "codex" ? "Codex" : "Anthropic API",
  configured: true, is_default: false, capabilities, ...extra,
});
const MODELS = [
  model("anthropic", "claude-opus-5-5", { auth_source: "ANTHROPIC_API_KEY (environment)" }),
  model("claude-code", "claude-haiku-5-5", { is_default: true, auth_source: "Claude Code login" }),
  model("codex", "gpt-6-luna"),
  model("gemini", "gemini-3", { configured: false, provider_name: "Gemini" }),
];
const provider = (id: string, name: string, engine: boolean): aether.ProviderInfo => ({
  provider: id, name, engine, configured: true, enabled: true, models: 1, open_model_list: false, protocol: engine ? "engine" : "anthropic",
  health: { provider: id } as aether.ProviderHealth, ...(engine ? {} : { auth_source: "stored credential" }),
});
/** The daemon's `providers`: two engine CLIs on subscriptions, one API provider with a stored key; gemini is not listed. */
const PROVIDERS = [provider("codex", "Codex", true), provider("claude-code", "Claude Code", true), provider("anthropic", "Anthropic API", false)];

describe("modelOptions", () => {
  test("tells subscription from API-billed by whether the daemon lists the provider as an engine, as Noesis does", () => {
    expect(billingOf(MODELS[0] as aether.ModelInfo, PROVIDERS)).toBe("api");
    expect(billingOf(MODELS[1] as aether.ModelInfo, PROVIDERS)).toBe("subscription");
    expect(billingOf(MODELS[2] as aether.ModelInfo, PROVIDERS)).toBe("subscription");
    expect(billingOf(MODELS[3] as aether.ModelInfo, PROVIDERS)).toBe("unknown");
    expect(billingOf(MODELS[0] as aether.ModelInfo, [])).toBe("unknown");
  });

  test("groups by provider, leaves unconfigured models out by default and hides API-billed ones on request", () => {
    const all = modelOptions(MODELS, PROVIDERS);
    expect(all.map((o) => [o.group, o.id, o.badge])).toEqual([
      ["Anthropic API", "anthropic/claude-opus-5-5", "API-billed"],
      ["Claude Code", "claude-code/claude-haiku-5-5", "Default"],
      ["Codex", "codex/gpt-6-luna", "Subscription"],
    ]);
    expect(modelOptions(MODELS, PROVIDERS, { allowApiBilled: false }).map((o) => o.id)).toEqual(["claude-code/claude-haiku-5-5", "codex/gpt-6-luna"]);
    const shown = modelOptions(MODELS, PROVIDERS, { showUnconfigured: true });
    expect(shown.find((o) => o.id === "gemini/gemini-3")).toMatchObject({ disabled: true, badge: "Not signed in" });
    expect(all[0]?.description).toBe("200k context");
  });

  test("while API billing is off, a model whose billing is not known yet is not offered either", () => {
    expect(modelOptions(MODELS, [], { allowApiBilled: false })).toEqual([]);
    expect(modelOptions(MODELS, [], { allowApiBilled: true }).map((o) => o.badge)).toEqual([undefined, "Default", undefined]);
  });
});

describe("ModelPicker", () => {
  test("offers the default option first and hands back the provider/model text", async () => {
    const chosen: string[] = [];
    const { container, unmount } = await render(
      <ModelPicker models={MODELS} providers={PROVIDERS} value="" defaultLabel="Default (first ready subscription model)" onChange={(ref) => chosen.push(ref)} />,
    );
    const trigger = container.querySelector("[role=combobox]") as HTMLButtonElement;
    expect(trigger.textContent).toContain("Default (first ready subscription model)");
    await click(trigger);
    const options = [...container.querySelectorAll<HTMLElement>("[role=option]")];
    expect(options[0]?.textContent).toContain("Default (first ready");
    await click(options.find((o) => o.textContent?.includes("gpt-6-luna")) as HTMLElement);
    expect(chosen).toEqual(["codex/gpt-6-luna"]);
    await unmount();
  });

  test("with no ready model it says where to sign in and is disabled", async () => {
    const { container, unmount } = await render(<ModelPicker models={[MODELS[3] as aether.ModelInfo]} value="" onChange={() => undefined} />);
    expect(container.textContent).toContain("Settings → Providers");
    expect((container.querySelector("[role=combobox]") as HTMLButtonElement).disabled).toBe(true);
    await unmount();
  });
});
