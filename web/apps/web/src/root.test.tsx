// @vitest-environment happy-dom
import { ToastRegion } from "@aether-ui/ui";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, test, vi } from "vitest";
import { reportNotice } from "./root.tsx";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("reports as toasts", () => {
  test("a report becomes a danger toast with its code as the body", () => {
    expect(reportNotice({ id: 1, message: "Lost the link", code: "timeout" }))
      .toEqual({ id: 1, title: "Lost the link", tone: "danger", body: "timeout" });
    expect(reportNotice({ id: 2, message: "Lost the link" })).not.toHaveProperty("body");
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
});
