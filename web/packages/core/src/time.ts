// Times as the screens tell them: a parsed stamp and how long ago it was.

import { useEffect, useState } from "react";

export function parseTime(at: string | null | undefined): number | null {
  if (typeof at !== "string") return null;
  const time = Date.parse(at);
  return Number.isNaN(time) ? null : time;
}

/** "just now", "5 min ago", "3 h ago", "2 d ago"; the raw text when it does not parse. */
export function relativeTime(at: string, now: number): string {
  const time = parseTime(at);
  if (time === null) return at;
  const minutes = Math.floor((now - time) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)} h ago`;
  return `${Math.floor(minutes / (24 * 60))} d ago`;
}

/** The clock as a screen reads it: now, then every `tickMs`, so relative times stay true. */
export function useNow(tickMs = 30_000): number {
  if (!Number.isFinite(tickMs) || tickMs < 1_000) throw new Error(`a clock ticks at least every second: ${tickMs}`);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), tickMs);
    return () => clearInterval(timer);
  }, [tickMs]);
  return now;
}
