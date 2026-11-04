// The desktop preferences of CONTRACTS.md §2.4, one JSON file in Electron's userData. Each key
// has its own check; remote.origins is stored normalized, and a removed origin's data is cleared.

import { readFileSync, renameSync, writeFileSync } from "node:fs";
import type { PreferenceKey } from "@aether-ui/protocol";
import { normalizeOriginList } from "../security/origins.ts";
import { BridgeRefusal } from "../security/validate.ts";

const MAX_LAYOUT_BYTES = 64 * 1024;
const THEMES = ["system", "light", "dark"];

function bounds(value: unknown): unknown {
  const box = value as Record<string, unknown> | null;
  const fields = ["x", "y", "width", "height"];
  if (typeof box !== "object" || box === null || Object.keys(box).sort().join() !== [...fields].sort().join()) {
    throw new BridgeRefusal("window.bounds is {x, y, width, height}");
  }
  const ok = fields.every((field) => Number.isSafeInteger(box[field]) && Math.abs(box[field] as number) <= 100_000)
    && (box.width as number) >= 200 && (box.height as number) >= 200;
  if (!ok) throw new BridgeRefusal("window.bounds holds whole pixels, at least 200 wide and high");
  return { x: box.x, y: box.y, width: box.width, height: box.height };
}

/** The value to store under `key`, checked and normalized, or a refusal. */
export function checkPreference(key: PreferenceKey, value: unknown): unknown {
  switch (key) {
    case "window.bounds": return bounds(value);
    case "layout.panels": {
      const text = JSON.stringify(value);
      if (text === undefined || Buffer.byteLength(text) > MAX_LAYOUT_BYTES) throw new BridgeRefusal("layout.panels is JSON under 64 KiB");
      return JSON.parse(text) as unknown;
    }
    case "theme":
      if (typeof value !== "string" || !THEMES.includes(value)) throw new BridgeRefusal(`theme is one of ${THEMES.join(", ")}`);
      return value;
    case "desktop.managed_start":
      if (typeof value !== "boolean") throw new BridgeRefusal("desktop.managed_start is true or false");
      return value;
    case "remote.origins":
      return normalizeOriginList(value);
  }
}

export class PreferenceStore {
  private readonly file: string;
  private readonly values: Map<PreferenceKey, unknown>;
  private readonly onOriginRemoved: (origin: string) => Promise<void>;

  constructor(file: string, onOriginRemoved: (origin: string) => Promise<void>) {
    this.file = file;
    this.onOriginRemoved = onOriginRemoved;
    this.values = new Map(Object.entries(PreferenceStore.load(file)) as Array<[PreferenceKey, unknown]>);
  }

  private static load(file: string): Record<string, unknown> {
    let text: string;
    try {
      text = readFileSync(file, "utf8");
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
      throw new Error(`Reading the desktop preferences at ${file}`, { cause: error });
    }
    const parsed: unknown = JSON.parse(text);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) throw new Error(`${file} is not a JSON object`);
    return parsed as Record<string, unknown>;
  }

  get(key: PreferenceKey): unknown {
    return this.values.get(key) ?? (key === "remote.origins" ? [] : null);
  }

  async set(key: PreferenceKey, value: unknown): Promise<void> {
    const checked = checkPreference(key, value);
    const before = key === "remote.origins" ? (this.get(key) as string[]) : [];
    this.values.set(key, checked);
    const temporary = `${this.file}.tmp`;
    writeFileSync(temporary, `${JSON.stringify(Object.fromEntries(this.values), null, 2)}\n`, { mode: 0o600 });
    renameSync(temporary, this.file);
    const kept = new Set(key === "remote.origins" ? (checked as string[]) : before);
    for (const origin of before.filter((origin) => !kept.has(origin))) await this.onOriginRemoved(origin);
  }
}
