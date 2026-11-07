// @vitest-environment happy-dom
// UX-043: a repair belongs to the app, so its progress and outcome show however the view that
// started it goes: replaced by another connection's page, or by the banner while Archon is away.

import { act, useEffect, useState } from "react";
import { afterEach, expect, test } from "vitest";
import { buttonNamed, click, connected, type Harness } from "../kit/fixture/harness.test-util.tsx";
import { fakeDesktop } from "../kit/fixture/desktop.test-util.ts";
import { ArchonToolsNotice, ConnectArchon } from "./ConnectArchon.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

type View = "banner" | "offer" | "remote";

function Views({ onSwap }: { readonly onSwap: (swap: (view: View) => void) => void }) {
  const [view, setView] = useState<View>("banner");
  useEffect(() => onSwap(setView), [onSwap]);
  if (view === "offer") return <ConnectArchon key="offer" needed reason="Archon's tools are not connected to Aether yet." />;
  if (view === "remote") return <ConnectArchon key="remote" needed={false} reason="" remote />;
  return <ArchonToolsNotice />;
}

test("a started repair keeps its progress and failure through a remote view and the banner", async () => {
  let fail: (error: Error) => void = () => undefined;
  const bridge = Object.assign(fakeDesktop().bridge, { setup: () => new Promise((_, reject) => { fail = reject; }) });
  const h = current = await connected({ role: "local", desktop: bridge });
  let swap: (view: View) => void = () => undefined;
  const root = await h.mount(<Views onSwap={(set) => { swap = set; }} />);
  const show = async (view: View) => { act(() => swap(view)); await h.settle(); };
  expect(root.querySelector("aside, .su-repair")).toBeNull();

  await show("offer");
  await click(h, root, "Connect Archon tools");
  expect(root.querySelector('.su-step[data-state="running"]')?.textContent).toContain("Connect Archon's tools to Aether");

  await show("remote");
  expect(root.querySelector('.su-step[data-state="running"]')?.textContent).toContain("Connect Archon's tools to Aether");
  expect(root.textContent).not.toContain("Connect them from the desktop app");

  await show("banner");
  expect(root.querySelector('aside[aria-label="Archon\'s tools"] .su-step[data-state="running"]')).not.toBeNull();
  fail(new Error("Archon could not be reached. Is it installed?"));
  await h.settle();
  expect(root.querySelector('.su-step[data-state="failed"]')?.textContent).toContain("Archon could not be reached. Is it installed?");
  expect(buttonNamed(root, "Try again").disabled).toBe(false);
});

test("nothing is offered when nothing is needed and no repair was started", async () => {
  const h = current = await connected({ role: "local", desktop: fakeDesktop().bridge });
  const root = await h.mount(<><ConnectArchon needed={false} reason="" /><ArchonToolsNotice /></>);
  expect(root.textContent).toBe("");
});
