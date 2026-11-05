// Noesis is off by default: every `noesis_*` command but status and configure is refused with
// `noesis_disabled` and changes nothing, no model is asked, and `noesis.db` is never made.

import { expect, test } from "../../harness/fixtures.ts";
import { noesisFiles, noesisStatus } from "./noesis.ts";

/** One well-formed instance of every gated command, so a refusal is the gate and not the parser. */
const GATED = [
  { cmd: "noesis_create_session", goal: "E2E goal", mode: "teach" },
  { cmd: "noesis_send", session: "ns_e2e", text: "hello" },
  { cmd: "noesis_answer", session: "ns_e2e", question: "nq_e2e", answer: { dont_know: true } },
  { cmd: "noesis_delete_session", session: "ns_e2e", graph: "preserve" },
  { cmd: "noesis_delete_proposition", proposition: "np_e2e" },
  { cmd: "noesis_delete_source", source: "nsrc_e2e", graph: "preserve" },
  { cmd: "noesis_extract", session: "ns_e2e" },
  { cmd: "noesis_import_source", artifact: "art_01ARZ3NDEKTSV4RRFFQ69G5FAV", name: "e2e.md" },
  { cmd: "noesis_cancel_job", job: "nj_e2e" },
  { cmd: "noesis_correct_learner", concept: "nc_e2e", state: "fragile", reason: "e2e" },
  { cmd: "noesis_get_session", session: "ns_e2e" },
  { cmd: "noesis_list_sessions" },
  { cmd: "noesis_graph" },
  { cmd: "noesis_jobs" },
  { cmd: "noesis_learner" },
  { cmd: "noesis_learner_evidence", concept: "nc_e2e" },
] as const;

test("with the default config every gated noesis command is refused as noesis_disabled, with no side effect", async ({ stack }) => {
  expect(await noesisFiles(stack), "noesis.db does not exist on a fresh daemon").toEqual([]);
  const before = await noesisStatus(stack);
  expect(before.enabled, "noesis.enabled is false by default").toBe(false);
  expect(before.settings.enabled, "the stored default is off").toBe(false);
  expect(before.jobs_running, "no Noesis job runs").toBe(0);
  const requests = stack.engine.requests.length;

  for (const command of GATED) {
    const refusal = await stack.aether.request(command).then((reply) => `answered ${JSON.stringify(reply)}`, (error: unknown) => String(error));
    expect(refusal, `${command.cmd} is refused as unavailable`).toMatch(/"code":"unavailable"/);
    expect(refusal, `${command.cmd} names the reason`).toMatch(/"reason":"noesis_disabled"/);
  }

  expect(await noesisStatus(stack), "the refusals changed no setting and started no job").toEqual(before);
  expect(stack.engine.requests.length, "no refused command reached the model").toBe(requests);
  expect(await noesisFiles(stack), "no refused command made noesis.db").toEqual([]);
});
