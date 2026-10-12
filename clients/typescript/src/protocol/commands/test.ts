// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.


/**
 * Send the calling device one `test` notification.
 */
export interface TestPushCommand {
  "cmd": "test_push";
}

export function testPush(): TestPushCommand {
  return { cmd: "test_push" };
}
