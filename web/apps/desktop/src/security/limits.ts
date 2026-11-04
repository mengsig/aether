// The size bounds of CONTRACTS.md §2.4: every IPC argument is checked against them in main.

/** The largest client frame the renderer may hand to main, as UTF-8 JSON. */
export const MAX_FRAME_BYTES = 1024 * 1024;

/** The longest string argument of any bridge method, in UTF-8 bytes. */
export const MAX_STRING_BYTES = 4 * 1024;

/** Descriptive labels (a device name) are cut to this many characters in a confirmation. */
export const MAX_LABEL_CHARS = 200;

/** A confirmation longer than this is refused rather than shown in part. */
export const MAX_CONFIRMATION_LINES = 60;

/** The most origins `remote.origins` may hold. */
export const MAX_REMOTE_ORIGINS = 16;

/** The longest line a daemon may send before main drops the connection. */
export const MAX_DAEMON_LINE_BYTES = 64 * 1024 * 1024;

export function utf8Bytes(text: string): number {
  return Buffer.byteLength(text, "utf8");
}
