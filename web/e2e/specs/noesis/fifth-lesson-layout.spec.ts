import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import path from "node:path";
import { expect, test } from "../../harness/fixtures.ts";
import { pairBoth } from "../../harness/pair.ts";
import type { Stack } from "../../harness/stack.ts";
import { configureNoesis } from "./noesis.ts";

const SESSION = "ns_fifth_lesson_layout";
const AGENT = "agt_00000000000000000000000000";
const AT = "2026-11-01T12:00:00Z";

function seedLesson(stack: Stack, remove: boolean): void {
  const db = new DatabaseSync(path.join(stack.layout.aether.data, "noesis.db"));
  try {
    db.exec("PRAGMA foreign_keys = ON");
    expect(db.prepare("DELETE FROM sessions WHERE id = ?").run(SESSION).changes).toBeLessThanOrEqual(1);
    if (remove) return;
    expect(db.prepare("INSERT INTO sessions (id, goal, mode, phase, agent, created_at, updated_at) VALUES (?, ?, 'teach', 'teach', ?, ?, ?)")
      .run(SESSION, "Understand the lesson", AGENT, AT, AT).changes).toBe(1);
    expect(db.prepare("INSERT INTO session_agents (session, ord, agent, role, started_at) VALUES (?, 0, ?, 'teacher', ?)")
      .run(SESSION, AGENT, AT).changes).toBe(1);
    const plan = { nodes: [{ id: "layout", title: "Understand the lesson" }], edges: [] };
    expect(db.prepare("INSERT INTO plans VALUES (?, 1, ?, ?)").run(SESSION, JSON.stringify(plan), AT).changes).toBe(1);
    expect(db.prepare("INSERT INTO questions (id, session, kind, prompt, answer_key, posed_at, answer, answered_at, plan_node, collapsed, correct) VALUES (?, ?, 'recall', ?, ?, ?, ?, ?, 'layout', 1, 1)")
      .run("q_fifth_lesson_layout", SESSION, "What did you learn?", JSON.stringify(["The lesson"]), AT, JSON.stringify({ text: "The lesson" }), AT).changes).toBe(1);
  } finally {
    db.close();
  }
}

test.beforeAll(async ({ stack }) => {
  await configureNoesis(stack, { enabled: true });
  seedLesson(stack, false);
});

test.afterAll(async ({ stack }) => {
  seedLesson(stack, true);
  await configureNoesis(stack, { enabled: false });
});

test("completion aligns with the lesson and contains its actions", async ({ page, stack }) => {
  const reply = await stack.aether.request({ cmd: "noesis_get_session", session: SESSION });
  expect(reply.reply, JSON.stringify(reply)).toBe("noesis_session");
  expect(reply.data.plan).toMatchObject({ nodes: [{ id: "layout", status: "visited" }] });
  await pairBoth(page, stack);
  const css = readFileSync(path.resolve(import.meta.dirname, "../../../packages/noesis/src/learn/learn.css"), "utf8");
  await page.route("**/fifth-lesson-layout.css", (route) => route.fulfill({ contentType: "text/css", body: css }));
  await page.goto(`${stack.aether.origin}/#/learn/sessions/${SESSION}`);
  // The daemon serves the committed bundle; exercise the source stylesheet before its next sync.
  await page.addStyleTag({ url: `${stack.aether.origin}/fifth-lesson-layout.css` });
  const card = page.getByRole("region", { name: "Lesson complete" });
  const lesson = page.locator(".learn-q");
  await expect(card).toBeVisible();
  await expect(lesson).toBeVisible();

  for (const viewport of [{ width: 1024, height: 768 }, { width: 1280, height: 800 }, { width: 3440, height: 1440 }]) {
    await page.setViewportSize(viewport);
    const cardBox = await card.boundingBox();
    const lessonBox = await lesson.boundingBox();
    assert.ok(cardBox && lessonBox, "the lesson and completion card have bounds");
    expect(Math.abs(cardBox.x - lessonBox.x)).toBeLessThanOrEqual(1);
    expect(Math.abs(cardBox.width - lessonBox.width)).toBeLessThanOrEqual(1);
    const actions = card.getByRole("button");
    await expect(actions).toHaveCount(3);
    for (const button of await actions.all()) {
      await expect(button).toBeVisible();
      const box = await button.boundingBox();
      assert.ok(box, "each action has bounds");
      expect(box.x).toBeGreaterThanOrEqual(cardBox.x - 1);
      expect(box.x + box.width).toBeLessThanOrEqual(cardBox.x + cardBox.width + 1);
    }
    const overflow = await page.evaluate(() => ({ document: document.documentElement.scrollWidth - document.documentElement.clientWidth,
      transcript: (document.querySelector(".learn-scroll") as HTMLElement).scrollWidth - (document.querySelector(".learn-scroll") as HTMLElement).clientWidth }));
    expect(overflow.document).toBeLessThanOrEqual(1);
    expect(overflow.transcript).toBeLessThanOrEqual(1);
  }
});
