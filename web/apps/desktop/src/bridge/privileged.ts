// Privileged actions (CONTRACTS.md §2.4): main builds the daemon frame from the validated action,
// writes the confirmation from its templates, and sends exactly the frame the user confirmed.

import type { PrivilegedAction } from "@aether-ui/protocol";
import {
  changesBlock, confirmation, daemonSlot, labelSlot, TEMPLATES, valueSlot,
  type Confirmation, type DaemonIdentity, type SettingChange,
} from "../security/confirm.ts";
import { archonChanges, archonOverlay, noesisChanges, noesisWirePatch } from "./settings-diff.ts";

/** Reads main needs before it can word or build a frame, on main's own control connection. */
export type Read = (frame: Readonly<Record<string, unknown>>) => Promise<unknown>;

export type Delivery =
  | { readonly kind: "frame"; readonly req: number; readonly text: string }
  | { readonly kind: "terminal"; readonly argv: readonly string[] };

/** One confirmed unit: what the dialog shows and the exact bytes or command it permits. */
export interface Plan {
  readonly confirmation: Confirmation;
  readonly delivery: Delivery;
}

export const LOGIN_COMMANDS = {
  claude: { name: "Claude", argv: ["claude", "auth", "login"] },
  codex: { name: "Codex", argv: ["codex", "login"] },
} as const;

const REJECT_REASON = "Rejected in the desktop app";

function frame(req: number, body: Readonly<Record<string, unknown>>): Delivery {
  if (!Number.isSafeInteger(req) || req < 1) throw new Error(`a request id is a positive integer, not ${req}`);
  if (typeof body.cmd !== "string") throw new Error("a daemon frame names its cmd");
  return { kind: "frame", req, text: JSON.stringify({ ...body, req }) };
}

interface RemoteView {
  revision: number;
  settings: Record<string, unknown>;
}

async function readRemote(read: Read): Promise<RemoteView> {
  const view = (await read({ cmd: "get_remote" })) as Partial<RemoteView> | undefined;
  if (!Number.isSafeInteger(view?.revision)) throw new Error("get_remote answered without a revision");
  if (typeof view?.settings !== "object" || view.settings === null) throw new Error("get_remote answered without settings");
  return view as RemoteView;
}

/** The settings whose frame depends on what the daemon holds now: remote access and mounts. */
async function remotePlan(action: PrivilegedAction, daemon: string, req: number, read: Read): Promise<Plan> {
  const view = await readRemote(read);
  const settings = { ...view.settings };
  if (action.kind === "remote.mount") {
    const mounts = Array.isArray(settings.mounts) ? (settings.mounts as Array<{ prefix?: unknown }>) : [];
    const target = `http://127.0.0.1:${action.target_port}`;
    settings.mounts = [...mounts.filter((mount) => mount.prefix !== action.prefix), { prefix: action.prefix, kind: "archon", target }];
    const shown = confirmation(TEMPLATES["remote.mount"], {
      prefix: valueSlot(action.prefix), port: String(action.target_port), daemon,
    });
    return { confirmation: shown, delivery: frame(req, { cmd: "configure_remote", settings, expected_revision: view.revision }) };
  }
  if (action.kind !== "remote.enable" && action.kind !== "remote.disable") throw new Error(`${action.kind} is not a remote action`);
  settings.enabled = action.kind === "remote.enable";
  return {
    confirmation: confirmation(TEMPLATES[action.kind], { daemon }),
    delivery: frame(req, { cmd: "configure_remote", settings, expected_revision: view.revision }),
  };
}

function settingsPlan(template: "noesis.configure" | "archon.settings", changes: SettingChange[], daemon: string,
  delivery: Delivery): Plan {
  return { confirmation: confirmation(TEMPLATES[template], { changes: changesBlock(changes), daemon }), delivery };
}

/**
 * The plan for `action` on `daemon`. `req` is the request id the frame will carry on main's
 * control connection, so the confirmed text is final.
 */
export async function planAction(action: PrivilegedAction, identity: DaemonIdentity, req: number, read: Read): Promise<Plan> {
  const daemon = daemonSlot(identity);
  switch (action.kind) {
    case "memory.accept":
      return { confirmation: confirmation(TEMPLATES[action.kind], { memory: valueSlot(action.memory), daemon }),
        delivery: frame(req, { cmd: "accept_memory", id: action.memory }) };
    case "memory.reject":
      return { confirmation: confirmation(TEMPLATES[action.kind], { memory: valueSlot(action.memory), daemon }),
        delivery: frame(req, { cmd: "reject_memory", id: action.memory, reason: REJECT_REASON }) };
    case "device.pair":
      return { confirmation: confirmation(TEMPLATES[action.kind], { name: labelSlot(action.name), role: action.role, daemon }),
        delivery: frame(req, { cmd: "start_pairing", role: action.role }) };
    case "device.revoke":
      return { confirmation: confirmation(TEMPLATES[action.kind], { device: valueSlot(action.device), daemon }),
        delivery: frame(req, { cmd: "revoke_device", id: action.device }) };
    case "remote.enable": case "remote.disable": case "remote.mount":
      return remotePlan(action, daemon, req, read);
    case "noesis.configure": {
      const changes = noesisChanges(await read({ cmd: "noesis_status" }), action.patch);
      return settingsPlan("noesis.configure", changes, daemon,
        frame(req, { cmd: "noesis_configure", patch: noesisWirePatch(action.patch) }));
    }
    case "archon.operator_change":
      return archonPlan(action, daemon, req, read);
    case "engine.login": {
      const login = LOGIN_COMMANDS[action.engine];
      return { confirmation: confirmation(TEMPLATES["engine.login"], {
        command: valueSlot(login.argv.join(" ")), engine: login.name, daemon,
      }), delivery: { kind: "terminal", argv: login.argv } };
    }
  }
}

type OperatorChange = Extract<PrivilegedAction, { kind: "archon.operator_change" }>;

async function archonPlan(action: OperatorChange, daemon: string, req: number, read: Read): Promise<Plan> {
  if (action.request === "settings") {
    const patch = action.patch as Record<string, unknown>;
    const view = await read({ cmd: "settings" });
    const settings = archonOverlay(view, patch);
    return settingsPlan("archon.settings", archonChanges(view, patch), daemon, frame(req, { cmd: "set_settings", settings }));
  }
  if (action.item === undefined) throw new Error(`${action.request} names an item`);
  const template = action.request === "merge" ? TEMPLATES["archon.merge"] : TEMPLATES["archon.unblock"];
  const cmd = action.request === "merge" ? "merge_item" : "retry_item";
  return { confirmation: confirmation(template, { item: valueSlot(action.item), daemon }), delivery: frame(req, { cmd, item: action.item }) };
}
