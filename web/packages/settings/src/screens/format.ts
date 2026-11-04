const STAMP: Intl.DateTimeFormatOptions = { dateStyle: "medium", timeStyle: "short" };

/** A daemon timestamp for people; the text itself when it is not a date. */
export function formatTime(iso: string | null | undefined): string {
  if (!iso) return "never";
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? iso : at.toLocaleString(undefined, STAMP);
}

export function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}
