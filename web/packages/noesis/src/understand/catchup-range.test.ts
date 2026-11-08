import { describe, expect, test } from "vitest";
import { FIRST_VISIT_DAYS, isSince, sinceFor, sinceInstant } from "./catchup-range.ts";

const NOW = new Date("2026-11-07T15:30:00Z");

describe("catch-up ranges", () => {
  test("a day is a real YYYY-MM-DD, not only shaped like one", () => {
    expect(isSince("2026-11-07")).toBe(true);
    for (const bad of ["2026-02-31", "2026-13-01", "26-11-07", "2026-11-07T00:00:00Z", "", "yesterday"]) expect(isSince(bad)).toBe(false);
    expect(sinceInstant("2026-11-07")).toBe(Date.parse("2026-11-07T00:00:00Z"));
  });

  test("the last 7 and 30 days start on that UTC day", () => {
    expect(sinceFor("week", NOW, null)).toBe("2026-10-31");
    expect(sinceFor("month", NOW, "2026-11-06T09:00:00Z")).toBe("2026-10-08");
  });

  test("since my last visit starts on the day of that visit", () => {
    expect(sinceFor("visit", NOW, "2026-11-03T23:59:00Z")).toBe("2026-11-03");
  });

  test("the first catch-up, an unreadable visit and one from the future all fall back to 7 days", () => {
    const week = sinceFor("week", NOW, null);
    expect(FIRST_VISIT_DAYS).toBe(7);
    for (const visit of [null, "not a time", "2027-01-01T00:00:00Z"]) expect(sinceFor("visit", NOW, visit)).toBe(week);
  });
});
