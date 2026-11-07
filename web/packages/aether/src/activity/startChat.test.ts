// Every transition of the chat start's state model, and the ones it refuses.

import { describe, expect, test } from "vitest";
import { FIRST_MESSAGE_LIMIT } from "./firstMessage.ts";
import { EDITING, firstMessageProblem, startChatStep, type ChatSettings, type StartChat, type StartEvent } from "./startChat.ts";

const settings: ChatSettings = { name: "", workspace: "/repo", model: { provider: "codex", model: "astra" }, modelName: "Astra", effort: "medium" };
const AGENT = "agt_01JSTART000000000000000001";
const run = (...events: StartEvent[]): StartChat => events.reduce(startChatStep, EDITING);
const create: StartEvent = { type: "create", settings, text: "/calm please" };
const refused: StartEvent = { type: "sent", result: { outcome: "refused", text: "/calm please", reason: "Inbox is full", keepProblem: null } };
const lost: StartEvent = { type: "sent", result: { outcome: "unconfirmed", text: "/calm please", reason: "No reply", keepProblem: null } };

describe("starting a chat", () => {
  test("validation names an empty or oversized first message before anything is created", () => {
    expect(firstMessageProblem("  ")).toBe("Write the first message");
    expect(firstMessageProblem(` ${"x".repeat(FIRST_MESSAGE_LIMIT)} `)).toBeNull();
    expect(firstMessageProblem("x".repeat(FIRST_MESSAGE_LIMIT + 1))).toContain("too long");
    expect(() => run({ ...create, text: "" })).toThrow();
    expect(() => run({ ...create, settings: { ...settings, workspace: "repo" } })).toThrow();
  });

  test("create, send and confirm keep the settings and text fixed throughout", () => {
    const sending = run(create, { type: "created", agent: AGENT });
    expect(sending).toEqual({ phase: "sending", agent: AGENT, settings, text: "/calm please" });
    expect(startChatStep(sending, { type: "sent", result: { outcome: "sent" } })).toEqual({ phase: "confirmed", agent: AGENT, settings, text: "/calm please" });
  });

  test("a failed create returns to editing; a lost one may have made the chat, so only a restart edits again", () => {
    expect(run(create, { type: "create-failed" })).toBe(EDITING);
    const lostCreate = run(create, { type: "create-lost", reason: "No reply" });
    expect(lostCreate).toMatchObject({ phase: "create-unconfirmed", reason: "No reply" });
    expect(() => startChatStep(lostCreate, create), "no second create from a lost one").toThrow();
    expect(startChatStep(lostCreate, { type: "restart" })).toBe(EDITING);
  });

  test.each([["refused", refused], ["unconfirmed", lost]] as const)("a %s first message retries the same agent with the same text", (phase, outcome) => {
    const failed = run(create, { type: "created", agent: AGENT }, outcome);
    expect(failed).toMatchObject({ phase, agent: AGENT, text: "/calm please" });
    expect(() => startChatStep(failed, create), "never a second create").toThrow();
    expect(startChatStep(failed, { type: "retry" })).toEqual({ phase: "sending", agent: AGENT, settings, text: "/calm please" });
    expect(startChatStep(failed, { type: "restart" })).toBe(EDITING);
  });

  test("transitions outside the model are programming errors", () => {
    expect(() => run({ type: "retry" })).toThrow();
    expect(() => run({ type: "created", agent: AGENT })).toThrow();
    expect(() => run(create, { type: "restart" })).toThrow();
    expect(() => run(create, { type: "created", agent: AGENT }, { type: "retry" })).toThrow();
    expect(() => run(create, { type: "created", agent: AGENT }, { type: "sent", result: { ...refused.result, text: "other" } } as StartEvent)).toThrow();
  });
});
