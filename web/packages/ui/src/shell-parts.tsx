import type { ConnectionInfo, ConnState } from "@aether-ui/protocol";
import { invariant } from "./classes.ts";
import { Menu } from "./menu.tsx";
import { THEME_PREFERENCES, type useTheme } from "./theme.tsx";

const STATE_LABELS: Readonly<Record<ConnState, string>> = {
  connecting: "Connecting", pairing: "Needs pairing", ready: "Connected", backoff: "Reconnecting", closed: "Offline",
};

/** Every daemon connection with its state in words, for the nav rail's footer. */
export function ConnectionList({ connections }: { readonly connections: readonly ConnectionInfo[] }) {
  if (connections.length === 0) return <p className="ui-conn-empty">No daemon connected</p>;
  return (
    <ul className="ui-conns" aria-label="Connections">
      {connections.map((connection) => {
        const state = STATE_LABELS[connection.state];
        invariant(state !== undefined, `unknown connection state ${connection.state}`);
        return (
          <li key={connection.id} className="ui-conn" data-state={connection.state}>
            <span className="ui-conn-dot" aria-hidden="true" />
            <span className="ui-conn-label">{connection.label}</span>
            <span className="ui-conn-kind">{connection.daemon}</span>
            <span className="ui-conn-state">{state}</span>
          </li>
        );
      })}
    </ul>
  );
}

const PREFERENCE_LABELS = { system: "Match the system", light: "Light", dark: "Dark" } as const;

export function ThemeMenu({ theme }: { readonly theme: NonNullable<ReturnType<typeof useTheme>> }) {
  const items = THEME_PREFERENCES.map((preference) => ({
    id: preference,
    label: `${preference === theme.preference ? "✓ " : ""}${PREFERENCE_LABELS[preference]}`,
    onSelect: () => theme.setPreference(preference),
  }));
  return <Menu label="Theme" items={items} variant="quiet" />;
}
