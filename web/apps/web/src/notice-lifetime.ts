// Each toast's lifetime: it leaves on its own NOTICE_MS after it first showed, on a clock of its
// own, so a failure that arrives later neither renews nor shortens the ones already showing.

import type { ToastMessage } from "@aether-ui/ui";
import { useEffect, useRef } from "react";
import { NOTICE_MS } from "./notices.ts";

type NoticeId = ToastMessage["id"];
type Timer = ReturnType<typeof setTimeout>;

/** Dismisses each notice NOTICE_MS after it appeared; a folded repeat comes under a new id and starts its own clock. */
export function useNoticeLifetime(notices: readonly ToastMessage[], dismiss: (id: NoticeId) => void): void {
  const timers = useRef(new Map<NoticeId, Timer>());
  useEffect(() => {
    const live = new Set(notices.map((notice) => notice.id));
    if (live.size !== notices.length) throw new Error("each notice has an id of its own");
    const running = timers.current;
    for (const [id, timer] of running) {
      if (live.has(id)) continue;
      clearTimeout(timer);
      running.delete(id);
    }
    for (const id of live) {
      if (running.has(id)) continue;
      running.set(id, setTimeout(() => { running.delete(id); dismiss(id); }, NOTICE_MS));
    }
  }, [notices, dismiss]);
  useEffect(() => {
    const running = timers.current;
    return () => {
      for (const timer of running.values()) clearTimeout(timer);
      running.clear();
    };
  }, []);
}
