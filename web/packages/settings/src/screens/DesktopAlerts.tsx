// Desktop alerts, on the desktop app only: which waits raise a native notification, whether they
// do while Aether is in front, and a test. These are this computer's own settings, kept in the
// app's storage, not the daemon's; push for paired devices stays below.

import { DEFAULT_ALERT_SETTINGS, INBOX_ROUTE, readAlertSettings, useApp, writeAlertSettings, type AlertSettings } from "@aether-ui/core";
import { Button } from "@aether-ui/ui";
import { useEffect, useState } from "react";
import { invariant } from "../kit/invariant.ts";
import { RunNote, Section, type RunStatus } from "../kit/kit.tsx";
import "./desktop-alerts.css";

type Choice = keyof AlertSettings;

const CHOICES: ReadonlyArray<{ readonly name: Choice; readonly label: string; readonly hint: string }> = [
  { name: "enabled", label: "Desktop alerts", hint: "A notification when an agent starts waiting on you. Clicking it opens the question." },
  { name: "questions", label: "Questions", hint: "An agent asks you something." },
  { name: "approvals", label: "Approvals", hint: "An agent waits for you to allow a tool call." },
  { name: "whileFocused", label: "Also while Aether is in front", hint: "Off: only when you are in another app." },
];

export const TEST_ALERT = {
  title: "Aether alerts are on",
  body: "This is how Aether tells you an agent needs you.",
  route: INBOX_ROUTE,
} as const;

function Toggle({ name, label, hint, checked, disabled, onChange }: {
  readonly name: Choice; readonly label: string; readonly hint: string; readonly checked: boolean;
  readonly disabled: boolean; readonly onChange: (name: Choice, on: boolean) => void;
}) {
  const hintId = `st-alert-${name}-hint`;
  return (
    <label className="st-toggle" data-disabled={disabled ? "true" : undefined}>
      <span className="st-toggle-text">
        <span className="st-toggle-label">{label}</span>
        <span className="st-toggle-hint" id={hintId}>{hint}</span>
      </span>
      <input type="checkbox" role="switch" className="st-switch" checked={checked} disabled={disabled}
        aria-describedby={hintId} onChange={(event) => onChange(name, event.currentTarget.checked)} />
    </label>
  );
}

export function DesktopAlerts() {
  const app = useApp();
  const desktop = app.host.desktop;
  invariant(desktop !== undefined, "desktop alerts are shown on the desktop app only");
  const [settings, setSettings] = useState<AlertSettings | null>(null);
  const [status, setStatus] = useState<RunStatus>({ state: "idle" });
  useEffect(() => {
    let live = true;
    readAlertSettings(app.host.storage).then((read) => { if (live) setSettings(read); },
      (error: unknown) => { if (live) { setSettings(DEFAULT_ALERT_SETTINGS); setStatus({ state: "failed", message: `Could not read the alert settings: ${String(error)}` }); } });
    return () => { live = false; };
  }, [app]);
  const change = (name: Choice, on: boolean) => {
    invariant(settings !== null, "settings are read before they change");
    const next = { ...settings, [name]: on };
    setSettings(next);
    writeAlertSettings(app.host.storage, next).then(
      () => setStatus({ state: "done", message: on ? `${labelOf(name)} on` : `${labelOf(name)} off` }),
      (error: unknown) => { setSettings(settings); setStatus({ state: "failed", message: `Not saved: ${String(error)}` }); });
  };
  const test = () => {
    desktop.notify(TEST_ALERT);
    setStatus({ state: "done", message: "Test notification sent. If none appeared, allow notifications for Aether in your system settings." });
  };
  return (
    <Section title="Desktop alerts" hint="Alerts from this computer's Aether app, without opening it.">
      <div className="st-toggles" aria-busy={settings === null}>
        {CHOICES.map((choice) => (
          <Toggle key={choice.name} {...choice} checked={settings?.[choice.name] ?? DEFAULT_ALERT_SETTINGS[choice.name]}
            disabled={settings === null || (choice.name !== "enabled" && !settings.enabled)} onChange={change} />
        ))}
      </div>
      <div className="st-row">
        <Button size="sm" variant="secondary" onClick={test} disabled={settings === null}>Send test notification</Button>
      </div>
      <RunNote status={status} />
    </Section>
  );
}

function labelOf(name: Choice): string {
  const choice = CHOICES.find((entry) => entry.name === name);
  invariant(choice !== undefined, `${name} is one of the alert choices`);
  return choice.label;
}
