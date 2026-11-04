import type { aether } from "@aether-ui/protocol";
import { Badge } from "@aether-ui/ui";
import { Action, RunNote, useRun } from "../controls/kit.tsx";
import { invariant } from "../fleet/invariant.ts";

export function MemoryRow({ conn, memory, refresh, proposal = false }: {
  conn: string; memory: aether.MemoryRecord; refresh: () => Promise<unknown>; proposal?: boolean;
}) {
  const run = useRun(conn);
  invariant(memory.id.length > 0, "memory has an identity");
  invariant(typeof memory.text === "string", "memory has text");
  const mutate = async (command: Record<string, unknown>) => { if (await run.send(command)) await refresh(); };
  const review = async (kind: "memory.accept" | "memory.reject") => {
    if (await run.privileged({ kind, memory: memory.id })) await refresh();
  };
  const pin = { cmd: "pin_memory", id: memory.id, pinned: !memory.pinned };
  const forget = { cmd: "forget_memory", id: memory.id };
  const restore = { cmd: "restore_memory", id: memory.id };
  return <article className="b3-item">
    <p className="b3-prose">{memory.text}</p>
    <div className="b3-row"><Badge>{memory.kind}</Badge><span className="b3-meta">{scopeText(memory.scope)}</span>
      {memory.pinned && <Badge>Pinned</Badge>}{memory.forgotten_at && <Badge>Forgotten</Badge>}
      {memory.superseded_by && <Badge>Superseded by {memory.superseded_by}</Badge>}</div>
    <p className="b3-meta">{memory.tags?.join(" · ")} · {memory.created_at}</p>
    <div className="b3-row">
      {proposal ? <>
        <Action conn={conn} command={{ cmd: "accept_memory", id: memory.id }} label="Accept" busy={run.status.state === "busy"} onRun={() => review("memory.accept")} />
        <Action conn={conn} command={{ cmd: "reject_memory", id: memory.id }} label="Reject" variant="danger" busy={run.status.state === "busy"}
          confirm={{ title: "Reject proposal?", body: "This proposal will not be accepted into memory.", confirmLabel: "Reject proposal" }} onRun={() => review("memory.reject")} />
      </> : <>
        <Action conn={conn} command={pin} label={memory.pinned ? "Unpin" : "Pin"} busy={run.status.state === "busy"} onRun={() => mutate(pin)} />
        {memory.forgotten_at ? <Action conn={conn} command={restore} label="Restore" busy={run.status.state === "busy"} onRun={() => mutate(restore)} /> :
          <Action conn={conn} command={forget} label="Forget" variant="danger" busy={run.status.state === "busy"} confirm={{ title: "Forget memory?", body: "It will no longer be used. You can restore it later.", confirmLabel: "Forget memory" }} onRun={() => mutate(forget)} />}
      </>}
    </div><RunNote status={run.status} />
  </article>;
}

/** "Global", or the repository a memory belongs to, in words. */
function scopeText(scope: aether.MemoryScope): string {
  switch (scope.scope) {
    case "global": return "Global";
    case "repository": return `Repository ${scope.id}`;
    case "role": return `Role ${scope.role}`;
    case "agent": return `Agent ${scope.id}`;
    case "repository_role": return `Repository ${scope.id} · role ${scope.role}`;
    case "subtree": return `Subtree of ${scope.root}`;
  }
}
