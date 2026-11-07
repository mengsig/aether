// The app window's native side, on the desktop only: OS links and notification clicks navigate,
// links to anywhere outside the app open through main with visible feedback, and new questions
// and approvals raise desktop alerts.

import { DesktopAlerts, INBOX_ROUTE, navigate, readAlertSettings, type App, type ReportTone } from "@aether-ui/core";
import type { DesktopBridge } from "@aether-ui/protocol";

type Say = (message: string, tone: ReportTone) => void;

const FOCUS_TRIES = 120;

/** The outside URL a click on `target` follows, or null for an in-app link or no link at all. */
export function outsideHref(target: EventTarget | null, base: string): string | null {
  const anchor = target instanceof Element ? target.closest("a[href]") : null;
  if (anchor === null) return null;
  const href = anchor.getAttribute("href") ?? "";
  if (href === "" || href.startsWith("#")) return null;
  const url = URL.parse(href, base);
  if (url === null) return href;
  const page = new URL(base);
  return url.protocol === page.protocol && url.host === page.host ? null : url.href;
}

/** What a person reads once a link opened, or failed to: a cancel in the native question says nothing. */
export async function openOutside(desktop: Pick<DesktopBridge, "openExternal">, href: string, say: Say): Promise<void> {
  const url = URL.parse(href);
  const where = url?.host || href.slice(0, 60);
  const report = url?.protocol === "http:";
  try {
    await desktop.openExternal(href);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message === "Cancelled") return;
    say(`Could not open ${report ? "the report" : where}: ${message}`, "danger");
    return;
  }
  say(report ? "The report opened in its own window" : `Opened ${where} in your browser`, "success");
}

/** Sends every click or middle-click on an outside link to main instead of a refused new window. */
export function interceptOutsideLinks(doc: Document, open: (href: string) => void): () => void {
  const onClick = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button > 1) return;
    const href = outsideHref(event.target, doc.baseURI);
    if (href === null) return;
    event.preventDefault();
    open(href);
  };
  doc.addEventListener("click", onClick);
  doc.addEventListener("auxclick", onClick);
  return () => {
    doc.removeEventListener("click", onClick);
    doc.removeEventListener("auxclick", onClick);
  };
}

/** Once the inbox shows the question an alert named, scrolls to it and puts the cursor in its answer. */
export function focusQuestion(doc: Document, anchor: string, frame: (step: () => void) => void, tries = FOCUS_TRIES): void {
  if (anchor === "" || tries < 0) throw new Error("a question anchor is not empty, with tries left");
  const entry = [...doc.querySelectorAll<HTMLElement>("[data-question]")].find((element) => element.dataset.question === anchor);
  if (entry === undefined) {
    if (tries > 0) frame(() => focusQuestion(doc, anchor, frame, tries - 1));
    return;
  }
  entry.scrollIntoView({ block: "center" });
  (entry.querySelector<HTMLElement>("textarea, button") ?? entry).focus();
}

/** A route main sent (an OS link or a notification click) becomes the app's own navigation. */
export function followDeepLinks(desktop: Pick<DesktopBridge, "onDeepLink">, alerts: DesktopAlerts, doc: Document): () => void {
  return desktop.onDeepLink((route) => {
    if (typeof route !== "string" || !route.startsWith("/")) throw new Error(`main sends app routes, not ${String(route)}`);
    navigate(`#${route}`);
    const anchor = route === INBOX_ROUTE ? alerts.takeAnchor() : undefined;
    if (anchor !== undefined) focusQuestion(doc, anchor, (step) => requestAnimationFrame(step));
  });
}

/** Connects the desktop's native surfaces to `app`; the returned function undoes it. */
export function connectDesktop(app: App, desktop: DesktopBridge, doc: Document): () => void {
  if (app.host.desktop !== desktop) throw new Error("the desktop bridge is the app host's own");
  const say: Say = (message, tone) => app.store.report(message, undefined, undefined, tone);
  const alerts = new DesktopAlerts({
    desktop,
    settings: () => readAlertSettings(app.host.storage),
    isFocused: () => doc.visibilityState === "visible" && doc.hasFocus(),
  });
  const undo = [
    followDeepLinks(desktop, alerts, doc),
    interceptOutsideLinks(doc, (href) => void openOutside(desktop, href, say)),
    alerts.watch(app.store, (message) => say(message, "danger")),
  ];
  return () => { for (const step of undo) step(); };
}
