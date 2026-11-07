import { describe, expect, test } from "vitest";
import { DraftError } from "./commands.ts";
import { scheduleCommand, type ScheduleDraft } from "./schedule-commands.ts";

const AGENT = "agt_01J0B3CONTROLS0000000000";

const draft = (over: Partial<ScheduleDraft> = {}): ScheduleDraft => ({
  name: "Morning check", target: "agent", recurrence: "interval", seconds: "3600", expression: "0 9 * * *", timezone: "Europe/Oslo",
  busy: "skip", missed: "coalesce", limit: "", message: "Check overnight CI", objective: "", workspace: "", model: "", effort: "", ...over,
});

const spawnDraft = (over: Partial<ScheduleDraft> = {}): ScheduleDraft =>
  draft({ target: "spawn", message: "", objective: " Triage the backlog ", workspace: "/srv/repo", model: "anthropic/claude-opus-5-5", ...over });

describe("scheduleCommand: delivery to an agent", () => {
  test("an interval schedule with the default policies", () => {
    expect(scheduleCommand(AGENT, draft())).toEqual({
      cmd: "create_schedule",
      spec: { name: "Morning check", target: AGENT, recurrence: { kind: "interval", seconds: 3600 }, payload: "Check overnight CI", busy: "skip", missed: "coalesce", max_deliveries: null },
    });
  });

  test("a cron schedule keeps its expression and timezone, with the chosen policies and limit", () => {
    const command = scheduleCommand(AGENT, draft({ recurrence: "cron", expression: "  0  9 * *   1-5 ", busy: "queue_one", missed: "skip", limit: "10" }));
    expect(command.spec).toEqual({
      name: "Morning check", target: AGENT, recurrence: { kind: "cron", expression: "0 9 * * 1-5", timezone: "Europe/Oslo" },
      payload: "Check overnight CI", busy: "queue_one", missed: "skip", max_deliveries: 10,
    });
  });

  test.each([
    ["a blank name", { name: " " }, "Name the schedule"],
    ["an interval under a minute", { seconds: "59" }, "The interval is at least 60 seconds"],
    ["a fractional interval", { seconds: "90.5" }, "The interval must be a whole number"],
    ["a blank interval", { seconds: "" }, "The interval must be a whole number"],
    ["a cron expression with four fields", { recurrence: "cron", expression: "0 9 * *" }, "five fields"],
    ["a blank timezone", { recurrence: "cron", timezone: " " }, "Name the timezone"],
    ["an unknown timezone", { recurrence: "cron", timezone: "Mars/Olympus" }, "not an IANA timezone"],
    ["an unknown busy policy", { busy: "queue_all" }, "agent is busy"],
    ["an unknown missed policy", { missed: "replay" }, "missed occurrences"],
    ["a zero limit", { limit: "0" }, "The maximum deliveries must be at least 1"],
    ["a negative limit", { limit: "-3" }, "The maximum deliveries must be a whole number"],
    ["a fractional limit", { limit: "2.5" }, "The maximum deliveries must be a whole number"],
    ["a blank message", { message: "  " }, "Write the message"],
  ] as const)("refuses %s", (_label, over, message) => {
    expect(() => scheduleCommand(AGENT, draft(over))).toThrow(DraftError);
    expect(() => scheduleCommand(AGENT, draft(over))).toThrow(message);
  });

  test("the cron timezone error keeps its cause", () => {
    try {
      scheduleCommand(AGENT, draft({ recurrence: "cron", timezone: "Mars/Olympus" }));
      expect.unreachable("an unknown timezone is refused");
    } catch (error) {
      expect(error).toBeInstanceOf(DraftError);
      expect((error as Error).cause).toBeInstanceOf(Error);
    }
  });

  test("a programmer mistake is not a draft error", () => {
    expect(() => scheduleCommand("", draft())).toThrow(/names its agent/);
    expect(() => scheduleCommand(AGENT, draft({ recurrence: "weekly" as never }))).not.toThrow(DraftError);
  });
});

describe("scheduleCommand: spawning a new agent", () => {
  test("sets a spawn template and no target, payload or busy policy", () => {
    expect(scheduleCommand(AGENT, spawnDraft({ recurrence: "cron", missed: "skip", limit: "7", effort: "x_high" }))).toEqual({
      cmd: "create_schedule",
      spec: {
        name: "Morning check", recurrence: { kind: "cron", expression: "0 9 * * *", timezone: "Europe/Oslo" }, missed: "skip", max_deliveries: 7,
        spawn: {
          role: "worker", objective: "Triage the backlog", model: { provider: "anthropic", model: "claude-opus-5-5" },
          scope: { workspace_root: "/srv/repo" }, approval: { require_for: ["external"] }, config: { effort: "x_high" },
        },
      },
    });
  });

  test("leaves the effort to the model when none is chosen", () => {
    const { spawn } = scheduleCommand(AGENT, spawnDraft()).spec;
    expect(spawn).not.toHaveProperty("config");
  });

  test.each([
    ["a blank objective", { objective: " " }, "Write the objective"],
    ["a relative workspace", { workspace: "repo" }, "absolute workspace path"],
    ["a blank workspace", { workspace: "" }, "absolute workspace path"],
    ["a model without a provider", { model: "claude-opus-5-5" }, "provider/model"],
    ["an unknown effort", { effort: "turbo" }, "valid effort"],
    ["an interval under a minute", { seconds: "10" }, "The interval is at least 60 seconds"],
  ] as const)("refuses %s", (_label, over, message) => {
    expect(() => scheduleCommand(AGENT, spawnDraft(over))).toThrow(DraftError);
    expect(() => scheduleCommand(AGENT, spawnDraft(over))).toThrow(message);
  });
});
