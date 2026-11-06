import { expect, test } from "vitest";
import { CRON_PRESETS, INTERVAL_PRESETS, describeCron, timezoneOptions } from "./schedule-presets.ts";

test("interval presets map to exact daemon seconds", () => {
  expect(INTERVAL_PRESETS.map(p => [p.label, Number(p.id)])).toEqual([
    ["1 minute", 60], ["5 minutes", 300], ["15 minutes", 900], ["30 minutes", 1800], ["Hourly", 3600], ["Daily", 86400],
  ]);
});
test("cron presets retain their expressions and explain their cadence", () => {
  expect(CRON_PRESETS.map(p => p.id)).toEqual(["0 9 * * 1-5", "0 * * * *", "0 9 * * *", "0 9 * * 1"]);
  expect(CRON_PRESETS.map(p => describeCron(p.id))).toEqual(["Every weekday at 09:00", "Every hour at minute 00", "Every day at 09:00", "Every Monday at 09:00"]);
});
test.each([
  ["30 8 * * *", "Every day at 08:30"], ["* * * * *", "Every minute"], ["15 * * * *", "Every hour at minute 15"],
  ["0 12 * * 0", "Every Sunday at 12:00"], ["0 12 * * 7", "Every Sunday at 12:00"],
  ["0 25 * * *", "Custom schedule: 0 25 * * *"], ["60 2 * * *", "Custom schedule: 60 2 * * *"],
  ["0 9 1 * *", "Custom schedule: 0 9 1 * *"], ["0 9 * 1 *", "Custom schedule: 0 9 * 1 *"],
])("describes %s without guessing unsupported expressions", (expression, text) => expect(describeCron(expression)).toBe(text));
test("incomplete cron input describes the fields needed", () => expect(describeCron("0 9")).toContain("five fields"));
test("timezone suggestions begin with the local zone and do not repeat it", () => {
  const zones = timezoneOptions("Europe/Oslo");
  expect(zones[0]?.id).toBe("Europe/Oslo");
  expect(zones.filter(z => z.id === "Europe/Oslo")).toHaveLength(1);
  expect(zones.some(z => z.id === "UTC")).toBe(true);
});
