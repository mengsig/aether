// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.



export interface UnsubscribeCommand {
  "cmd": "unsubscribe";
  "id": number;
}

export function unsubscribe(args: Omit<UnsubscribeCommand, "cmd">): UnsubscribeCommand {
  return { cmd: "unsubscribe", ...args };
}

/**
 * Forget the calling device's push subscription.
 */
export interface UnsubscribePushCommand {
  "cmd": "unsubscribe_push";
}

export function unsubscribePush(): UnsubscribePushCommand {
  return { cmd: "unsubscribe_push" };
}
