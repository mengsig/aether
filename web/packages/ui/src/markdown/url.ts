import { invariant } from "../classes.ts";

const MAX_URL_LENGTH = 2048;
const SAFE_SCHEMES = new Set(["http", "https", "mailto"]);
const NAMED_ENTITIES: Readonly<Record<string, string>> = { tab: "\t", newline: "\n", colon: ":", amp: "&", sol: "/" };
const ENTITY = /&(?:#x([0-9a-f]{1,6})|#(\d{1,7})|([a-z]{2,8}));?/gi;
const URL_SCHEME = /^([a-z][a-z0-9+.-]*):/i;

// ---------------------------------------------------------------------------------- URLs

function isInvisible(code: number): boolean {
  return code <= 0x20 || (code >= 0x7f && code <= 0xa0) || code === 0x1680 || (code >= 0x2000 && code <= 0x200f)
    || (code >= 0x2028 && code <= 0x202f) || (code >= 0x205f && code <= 0x2064) || code === 0x3000 || code === 0xfeff;
}

function decodeEntities(text: string): string {
  return text.replace(ENTITY, (whole, hex: string | undefined, dec: string | undefined, name: string | undefined) => {
    if (name !== undefined) return NAMED_ENTITIES[name.toLowerCase()] ?? whole;
    const code = hex !== undefined ? Number.parseInt(hex, 16) : Number.parseInt(dec ?? "", 10);
    return code <= 0x10ffff ? String.fromCodePoint(code) : whole;
  });
}

function stripInvisible(text: string): string {
  let clean = "";
  for (let i = 0; i < text.length; i++) if (!isInvisible(text.charCodeAt(i))) clean += text[i];
  return clean;
}

/** The normalized URL for http, https or mailto; otherwise null. */
export function safeHref(url: string): string | null {
  invariant(typeof url === "string", "safeHref takes a string");
  if (url.length > MAX_URL_LENGTH) return null;
  const clean = stripInvisible(decodeEntities(url));
  invariant(clean.length <= url.length, "normalising a URL never lengthens it");
  const scheme = URL_SCHEME.exec(clean)?.[1];
  return scheme !== undefined && SAFE_SCHEMES.has(scheme.toLowerCase()) ? clean : null;
}
