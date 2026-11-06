// Reports as the shell shows them: one toast per distinct failure, the newest few, each gone
// after a while or when dismissed. The reports themselves stay in the store.

import type { Report } from "@aether-ui/core";
import type { ToastMessage } from "@aether-ui/ui";

/** How many distinct failures show at once. */
export const VISIBLE_NOTICES = 4;
/** How long a toast stays once its newest report arrived. */
export const NOTICE_MS = 12_000;

const UNEXPLAINED_REPORT = "Something failed without saying why";

/** A report as a toast. Daemons may send a blank message, and a toast must have a title. */
export function reportNotice(report: Report, times = 1): ToastMessage {
  if (!Number.isInteger(times) || times < 1) throw new Error(`a notice stands for at least one report: ${times}`);
  const title = report.message.trim() === "" ? UNEXPLAINED_REPORT : report.message;
  const repeat = times > 1 ? `${times} times` : null;
  const body = [report.code, repeat].filter((part): part is string => part !== null && part !== undefined).join(" · ");
  return { id: report.id, title, tone: "danger", ...(body === "" ? {} : { body }) };
}

const sameFailure = (report: Report): string => `${report.message.trim()}\n${report.code ?? ""}`;

/**
 * The toasts for `reports`: identical failures fold into one toast under the newest report's id,
 * saying how many times they came. Dismissing it hides every report it folded; a later repeat
 * shows again and counts from there. Newest last.
 */
export function noticesOf(reports: readonly Report[], dismissed: ReadonlySet<ToastMessage["id"]>): ToastMessage[] {
  const groups = new Map<string, { newest: Report; times: number }>();
  for (const report of reports) {
    const key = sameFailure(report);
    const group = groups.get(key);
    if (dismissed.has(report.id)) { groups.delete(key); continue; }
    if (group === undefined) groups.set(key, { newest: report, times: 1 });
    else { group.newest = report.id > group.newest.id ? report : group.newest; group.times += 1; }
  }
  const notices = [...groups.values()]
    .sort((a, b) => a.newest.id - b.newest.id)
    .slice(-VISIBLE_NOTICES)
    .map((group) => reportNotice(group.newest, group.times));
  if (new Set(notices.map((notice) => notice.id)).size !== notices.length) throw new Error("each notice has its own report id");
  return notices;
}
