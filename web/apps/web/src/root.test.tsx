// @vitest-environment happy-dom
import { ToastRegion } from "@aether-ui/ui";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, test, vi } from "vitest";
import { noticesOf, reportNotice, VISIBLE_NOTICES } from "./notices.ts";
import { bannerFor } from "./root.tsx";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("reports as toasts", () => {
  test("a report becomes a danger toast with its code as the body", () => {
    expect(reportNotice({ id: 1, message: "Lost the link", code: "timeout" }))
      .toEqual({ id: 1, title: "Lost the link", tone: "danger", body: "timeout" });
    expect(reportNotice({ id: 2, message: "Lost the link" })).not.toHaveProperty("body");
  });

  test("UX-008: a report that names its tone keeps it, so a success is not shown as an error", () => {
    expect(reportNotice({ id: 3, message: "Started Astra", tone: "success" })).toEqual({ id: 3, title: "Started Astra", tone: "success" });
    expect(reportNotice({ id: 4, message: "Heads up", tone: "info" }).tone).toBe("info");
  });

  test("Archon's screens carry the Archon tools notice, and other screens no banner", () => {
    expect(bannerFor("#/archon/queue")).not.toBeNull();
    expect(bannerFor("#/archon")).not.toBeNull();
    expect(bannerFor("#/activity")).toBeNull();
    expect(bannerFor("#/archonx")).toBeNull();
  });

  test("a blank report message still renders, with a fallback title", async () => {
    const notices = [reportNotice({ id: 1, message: "" }), reportNotice({ id: 2, message: "  ", code: "incompatible" })];
    expect(notices.map((notice) => notice.title)).toEqual(["Something failed without saying why", "Something failed without saying why"]);
    const container = document.body.appendChild(document.createElement("div"));
    const root = createRoot(container);
    await act(async () => root.render(<ToastRegion toasts={notices} onDismiss={vi.fn()} />));
    expect(container.querySelectorAll("[role=alert]")).toHaveLength(2);
    await act(async () => root.unmount());
  });

  test("identical failures fold into one toast that says how many times they came", () => {
    const refused = { message: "agent agt_1 is completed; only live or failed agents can be updated", code: "invalid_argument" };
    const reports = [{ id: 1, ...refused }, { id: 2, ...refused }, { id: 3, message: "Lost the link", code: "timeout" }, { id: 4, ...refused }];
    expect(noticesOf(reports, new Set())).toEqual([
      { id: 3, title: "Lost the link", tone: "danger", body: "timeout" },
      { id: 4, title: refused.message, tone: "danger", body: "invalid_argument · 3 times" },
    ]);
  });

  test("dismissing a folded toast hides every report in it until the failure comes again", () => {
    const reports = [{ id: 1, message: "boom" }, { id: 2, message: "boom" }];
    expect(noticesOf(reports, new Set([2]))).toEqual([]);
    expect(noticesOf([...reports, { id: 3, message: "boom" }], new Set([2]))).toEqual([{ id: 3, title: "boom", tone: "danger" }]);
    expect(noticesOf([...reports, { id: 3, message: "boom" }, { id: 4, message: "boom" }], new Set([2]))).toEqual([{ id: 4, title: "boom", tone: "danger", body: "2 times" }]);
  });

  test("only the newest few distinct failures show", () => {
    const reports = Array.from({ length: VISIBLE_NOTICES + 2 }, (_, index) => ({ id: index + 1, message: `failure ${index + 1}` }));
    expect(noticesOf(reports, new Set()).map((notice) => notice.id)).toEqual([3, 4, 5, 6]);
  });

  test("Dismiss all is offered once more than one toast shows, and clears them", async () => {
    const onDismissAll = vi.fn();
    const container = document.body.appendChild(document.createElement("div"));
    const root = createRoot(container);
    const one = [reportNotice({ id: 1, message: "one" })];
    await act(async () => root.render(<ToastRegion toasts={one} onDismiss={vi.fn()} onDismissAll={onDismissAll} />));
    expect(container.textContent).not.toContain("Dismiss all");
    await act(async () => root.render(<ToastRegion toasts={[...one, reportNotice({ id: 2, message: "two" })]} onDismiss={vi.fn()} onDismissAll={onDismissAll} />));
    const clear = [...container.querySelectorAll("button")].find((button) => button.textContent === "Dismiss all");
    expect(clear).toBeDefined();
    await act(async () => { clear?.click(); });
    expect(onDismissAll).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
  });
});
