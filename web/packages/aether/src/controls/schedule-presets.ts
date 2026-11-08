import { invariant } from "../fleet/invariant.ts";

export const INTERVAL_PRESETS = [
  { id: "60", label: "1 minute" }, { id: "300", label: "5 minutes" }, { id: "900", label: "15 minutes" },
  { id: "1800", label: "30 minutes" }, { id: "3600", label: "Hourly" }, { id: "86400", label: "Daily" },
];
export const CRON_PRESETS = [
  { id: "0 9 * * 1-5", label: "Every weekday at 09:00" }, { id: "0 * * * *", label: "Every hour" },
  { id: "0 9 * * *", label: "Every day at 09:00" }, { id: "0 9 * * 1", label: "Every Monday at 09:00" },
];

export function describeCron(expression: string): string {
  invariant(typeof expression === "string", "a cron expression is text");
  const fields = expression.trim().split(/\s+/);
  if (fields.length !== 5) return "Use five fields: minute, hour, day, month, weekday";
  const [minute, hour, day, month, weekday] = fields;
  invariant(minute !== undefined && hour !== undefined && weekday !== undefined, "five cron fields were parsed");
  if (day !== "*" || month !== "*") return `Custom schedule: ${expression.trim()}`;
  if (minute === "*" && hour === "*" && weekday === "*") return "Every minute";
  if (/^\d+$/.test(minute) && Number(minute) < 60) {
    if (hour === "*" && weekday === "*") return `Every hour at minute ${minute.padStart(2, "0")}`;
    if (/^\d+$/.test(hour) && Number(hour) < 24) {
      const time = `${hour.padStart(2, "0")}:${minute.padStart(2, "0")}`;
      if (weekday === "*") return `Every day at ${time}`;
      if (weekday === "1-5") return `Every weekday at ${time}`;
      const names: Record<string, string> = { "0": "Sunday", "1": "Monday", "2": "Tuesday", "3": "Wednesday", "4": "Thursday", "5": "Friday", "6": "Saturday", "7": "Sunday" };
      if (names[weekday]) return `Every ${names[weekday]} at ${time}`;
    }
  }
  return `Custom schedule: ${expression.trim()}`;
}

export function timezoneOptions(local = Intl.DateTimeFormat().resolvedOptions().timeZone) {
  invariant(local.length > 0, "the local timezone has a name");
  const zones = [...new Set([local, "UTC", ...Intl.supportedValuesOf("timeZone")])];
  invariant(zones[0] === local, "the local timezone is first");
  return zones.map(id => ({ id, label: id }));
}
