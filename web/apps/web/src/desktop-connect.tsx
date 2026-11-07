// The pairing route inside the desktop app: a local daemon connects over the bridge by itself,
// and another computer opens in its own window, where the host's web UI pairs as usual.

import { useRoutes } from "@aether-ui/core";
import type { DesktopBridge } from "@aether-ui/protocol";
import { Badge, Button, ErrorState, Field } from "@aether-ui/ui";
import { useId, useState, type FormEvent, type ReactNode } from "react";

export const ADDRESS_EXAMPLE = "https://host.example.com";
// The bridge's string bound and origin shape, as main's normalizeOrigin checks them.
const MAX_ADDRESS_BYTES = 4 * 1024;
const BARE_ORIGIN = /^https:\/\/[^/?#\\@\s]+\/?$/i;
const IPC_PREFIX = /^Error invoking remote method '[^']*': (?:\w+: )?/;

function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`Invariant failed: ${message}`);
}

export type AddressCheck = { readonly origin: string } | { readonly error: string };

/** The origin a typed host address names; a bare host gets https://, anything beyond an origin is refused. */
export function checkHostAddress(typed: string): AddressCheck {
  const text = typed.trim();
  if (text === "") return { error: `Type the host's address, such as ${ADDRESS_EXAMPLE}.` };
  const value = /^[a-z][a-z0-9+.-]*:\/\//i.test(text) ? text : `https://${text}`;
  if (new TextEncoder().encode(value).length > MAX_ADDRESS_BYTES) return { error: "That address is too long." };
  if (!URL.canParse(value)) return { error: `That is not a host address. Type one such as ${ADDRESS_EXAMPLE}.` };
  const url = new URL(value);
  if (url.protocol !== "https:") return { error: `Another computer is reached over https: only, not ${url.protocol}` };
  if (url.username !== "" || url.password !== "" || value.includes("@")) return { error: "Leave out the user name and password: a host address has none." };
  if (!BARE_ORIGIN.test(value) || url.pathname !== "/" || url.search !== "" || url.hash !== "") {
    return { error: "Type only the host's address, without a path, query or fragment." };
  }
  invariant(url.origin.startsWith("https://"), "a checked address is an https origin");
  return { origin: url.origin };
}

/** What main refused with, without Electron's "Error invoking remote method" wrapping. */
export function bridgeMessage(error: unknown): string {
  const text = (error instanceof Error ? error.message : String(error)).replace(IPC_PREFIX, "").trim();
  return text === "" ? "The desktop app refused without giving a reason." : text;
}

/** Adds `origin` to remote.origins, keeping every saved entry and listing none twice. */
async function allowOrigin(desktop: DesktopBridge, origin: string): Promise<void> {
  const checked = checkHostAddress(origin);
  invariant("origin" in checked && checked.origin === origin, "only a checked origin is saved");
  const saved = (await desktop.getPreference("remote.origins")) ?? [];
  if (!Array.isArray(saved) || !saved.every((entry) => typeof entry === "string")) {
    throw new Error("The saved list of other computers is not a list of addresses. Check Settings → Connections.");
  }
  if (saved.includes(origin)) return;
  await desktop.setPreference("remote.origins", [...saved, origin]);
}

type Place = "this" | "another";
type Attempt =
  | { readonly phase: "idle" }
  | { readonly phase: "opening" | "opened"; readonly origin: string }
  | { readonly phase: "failed"; readonly origin: string; readonly message: string };

/** `#/pair/:daemon` on the desktop: choose this computer or another one; no code is typed here. */
export function DesktopConnect({ desktop, name }: { readonly desktop: DesktopBridge; readonly name: string }) {
  invariant(desktop.version === 1, "the desktop bridge is version 1");
  invariant(name.trim() !== "", "the daemon has a display name");
  const [place, setPlace] = useState<Place | null>(null);
  const group = useId();
  return (
    <section className="app-pair app-pair-desktop" aria-labelledby={`${group}-title`}>
      <p className="ui-eyebrow app-eyebrow">Connect to {name}</p>
      <h2 className="app-pair-title" id={`${group}-title`}>Where does {name} run?</h2>
      <fieldset className="app-pair-choices">
        <legend className="ui-visually-hidden">Where {name} runs</legend>
        <Choice group={group} value="this" chosen={place} onChoose={setPlace} title="This computer" badge="No code needed">
          The desktop app finds {name} here and connects by itself.
        </Choice>
        <Choice group={group} value="another" chosen={place} onChoose={setPlace} title="Another computer" badge="Opens its own window">
          Open the host's address in its own window and pair there.
        </Choice>
      </fieldset>
      {place === "this" ? <ThisComputer name={name} /> : null}
      {place === "another" ? <AnotherComputer desktop={desktop} name={name} /> : null}
    </section>
  );
}

interface ChoiceProps {
  readonly group: string;
  readonly value: Place;
  readonly chosen: Place | null;
  readonly onChoose: (place: Place) => void;
  readonly title: string;
  readonly badge: string;
  readonly children: ReactNode;
}

function Choice({ group, value, chosen, onChoose, title, badge, children }: ChoiceProps) {
  const bodyId = `${group}-${value}-body`;
  return (
    <label className="app-pair-choice">
      <input type="radio" name={group} value={value} checked={chosen === value} onChange={() => onChoose(value)}
        aria-label={title} aria-describedby={bodyId} className="app-pair-choice-input" />
      <span className="app-pair-choice-head">
        <span className="app-pair-choice-title">{title}</span>
        <Badge tone={chosen === value ? "accent" : "neutral"}>{badge}</Badge>
      </span>
      <span className="app-pair-choice-body" id={bodyId}>{children}</span>
    </label>
  );
}

function ThisComputer({ name }: { readonly name: string }) {
  const hasSetup = useRoutes().some((route) => route.path === "#/setup");
  return (
    <div className="app-pair-panel">
      <p className="app-pair-lede">{name} on this computer needs no pairing: once it runs, the app connects to it over the desktop bridge.</p>
      <a className="ui-button app-pair-link" data-variant="primary" href={hasSetup ? "#/setup" : "#/settings/connections"}>
        {hasSetup ? `Set up ${name} on this computer` : "Open Connections"}
      </a>
    </div>
  );
}

function AnotherComputer({ desktop, name }: { readonly desktop: DesktopBridge; readonly name: string }) {
  const [address, setAddress] = useState("");
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState<Attempt>({ phase: "idle" });
  const open = async (origin: string) => {
    setAttempt({ phase: "opening", origin });
    try {
      await allowOrigin(desktop, origin);
      await desktop.openRemote(origin);
      setAttempt({ phase: "opened", origin });
    } catch (error) {
      setAttempt({ phase: "failed", origin, message: bridgeMessage(error) });
    }
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const checked = checkHostAddress(address);
    if ("error" in checked) {
      setFieldError(checked.error);
      return;
    }
    setFieldError(null);
    void open(checked.origin);
  };
  const busy = attempt.phase === "opening";
  return (
    <form className="app-pair-panel" onSubmit={submit} aria-busy={busy} noValidate>
      <Field label="Host address" placeholder={ADDRESS_EXAMPLE} value={address} inputMode="url" autoComplete="url" spellCheck={false}
        hint={`The address where ${name}'s web UI is published, such as ${ADDRESS_EXAMPLE}.`}
        onChange={(event) => setAddress(event.currentTarget.value)} {...(fieldError === null ? {} : { error: fieldError })} />
      <Button type="submit" variant="primary" disabled={busy}>{busy ? "Opening…" : "Open the host"}</Button>
      <div role="status" className="app-pair-status">
        {busy ? `Opening ${attempt.origin}…` : null}
        {attempt.phase === "opened" ? (
          <p className="app-pair-opened"><span aria-hidden="true"><Badge tone="success">Window open</Badge></span> Opened {attempt.origin} in its own window. Pair there with the code the host shows.</p>
        ) : null}
      </div>
      {attempt.phase === "failed" ? (
        <ErrorState className="app-pair-error" title={`Could not open ${attempt.origin}`} message={attempt.message}
          retryLabel="Retry" onRetry={() => void open(attempt.origin)} />
      ) : null}
    </form>
  );
}
