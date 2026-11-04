// A fake desktop bridge: records the privileged actions core forwards and answers like the native
// confirmation would, so a test can check what was sent and that no reply is rendered.

import type { DesktopBridge, PrivilegedAction } from "@aether-ui/protocol";

export interface FakeDesktop {
  readonly bridge: DesktopBridge;
  readonly actions: PrivilegedAction[];
}

export function fakeDesktop(reply: unknown = { reply: "ok" }): FakeDesktop {
  const actions: PrivilegedAction[] = [];
  const bridge = {
    version: 1,
    connect: async () => "bridge-1",
    close: async () => {},
    privileged: async (action: PrivilegedAction) => { actions.push(action); return reply; },
    getPreference: async () => false,
  } as unknown as DesktopBridge;
  return { bridge, actions };
}
