/** @param {unknown} value @param {string} [label] @returns {Record<string,unknown>} */
export function object(value, label = 'value') {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${label} must be an object`);
  const result = /** @type {Record<string,unknown>} */ (value);
  if (Object.hasOwn(result, '__proto__')) throw new Error(`${label} contains an unsafe key`);
  return result;
}
/** @param {unknown} value @param {string} [label] */
export function string(value, label = 'value') {
  if (typeof value !== 'string' || !value.length) throw new Error(`${label} must be a nonempty string`);
  if (value.length > 2_000_000) throw new Error(`${label} exceeds the text limit`);
  return value;
}
/** @param {unknown} value @param {string} [label] */
export function integer(value, label = 'value') {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) throw new Error(`${label} must be a safe nonnegative integer`);
  return value;
}
/** @param {unknown} value @param {string} [label] @returns {unknown[]} */
export function array(value, label = 'value') {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  if (value.length > 100_000) throw new Error(`${label} exceeds the item limit`);
  return value;
}
/** @param {unknown} value */
export function event(value) {
  const item = object(value, 'event');
  if (!integer(item.seq, 'event.seq')) throw new Error('Event sequence must be positive');
  string(item.at, 'event.at'); string(object(item.kind).type, 'event.kind.type');
  if (item.agent_id != null) string(item.agent_id, 'event.agent_id');
  return item;
}
/** @param {unknown} raw */
export function frame(raw) {
  if (typeof raw !== 'string' || raw.length > 67_108_864 || new TextEncoder().encode(raw).length > 67_108_864) throw new Error('Invalid or oversized WebSocket text frame');
  const item = object(JSON.parse(raw), 'frame');
  if (item.frame === 'challenge') {
    string(item.instance_id,'challenge.instance_id');
    if (!/^[A-Za-z0-9_-]{43}$/.test(string(item.nonce))) throw new Error('Invalid challenge nonce');
  } else if (item.frame === 'reply') {
    integer(item.id, 'reply.id');
    const result = object(item.result, 'reply.result');
    if (Object.hasOwn(result, 'Ok') === Object.hasOwn(result, 'Err')) throw new Error('Reply must contain exactly one result');
    const body = object(result.Ok ?? result.Err);
    string(result.Ok ? body.reply : body.code, 'reply tag');
    if (result.Err) string(body.message, 'error.message');
  } else if (item.frame === 'event') {
    integer(item.sub, 'event.sub'); event(item.event);
  } else if (item.frame === 'delta') {
    integer(item.sub, 'delta.sub'); string(item.agent); string(item.invocation);
    const delta = object(item.delta);
    if (!['text', 'thinking', 'tool_input', 'turn_started', 'turn_finished'].includes(string(delta.delta))) throw new Error('Unknown delta');
    if (delta.delta === 'text' || delta.delta === 'thinking') {
      if (typeof delta.text !== 'string') throw new Error('Delta text must be a string');
    }
    if (delta.delta === 'turn_started') integer(delta.turn);
    if (delta.delta === 'tool_input') { string(delta.name); if (typeof delta.partial_json !== 'string') throw new Error('Invalid tool delta'); }
  } else throw new Error('Unknown server frame');
  return item;
}
/** @param {unknown} error */
export const errorText = error => error instanceof Error ? error.message : String(error);
