import {string} from './validate.js';

// Keep these ranges aligned with crates/aether-remote/src/auth/name.rs.
const UNSEEN = [
  [0x00ad,0x00ad],[0x034f,0x034f],[0x0600,0x0605],[0x061c,0x061c],[0x06dd,0x06dd],
  [0x070f,0x070f],[0x0890,0x0891],[0x08e2,0x08e2],[0x115f,0x1160],[0x17b4,0x17b5],
  [0x180b,0x180f],[0x200b,0x200f],[0x202a,0x202e],[0x2060,0x206f],[0x2800,0x2800],
  [0x3164,0x3164],[0xd800,0xdfff],[0xe000,0xf8ff],[0xfdd0,0xfdef],[0xfe00,0xfe0f],
  [0xfeff,0xfeff],[0xffa0,0xffa0],[0xfff0,0xfffc],[0x110bd,0x110bd],[0x110cd,0x110cd],
  [0x13430,0x1343f],[0x1bca0,0x1bca3],[0x1d173,0x1d17a],[0xe0000,0xe0fff],[0xf0000,0x10ffff],
];

/** @param {string} input */
export function normalizeDeviceName(input) {
  string(input,'Device name');
  const characters = [...input];
  for (const character of characters) {
    const point = /** @type {number} */ (character.codePointAt(0));
    if (point < 0x20 || (point >= 0x7f && point <= 0x9f) || (point & 0xfffe) === 0xfffe || UNSEEN.some(([first,last])=>point>=first && point<=last)) {
      throw new Error('Device name cannot contain control, formatting or invisible characters. Use plain text or an emoji without a joiner or variation selector.');
    }
  }
  const normalized = input.trim().replace(/\s+/gu,' ');
  if (!normalized || [...normalized].length>64) throw new Error('Device name must be 1–64 characters after spaces are combined.');
  return normalized;
}
