// Model providers, their sign-in sources and MCP servers, all derived from core's store. Nothing
// here asks for or shows a credential: signing in opens a sign-in window on the computer running Aether.

import { useApp, useReread } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { Badge, EmptyState, type Tone } from "@aether-ui/ui";
import { invariant } from "../kit/invariant.ts";
import { Action, RunNote, Section, StandingNotice, useRun, type Run } from "../kit/kit.tsx";
import { ConnectArchon } from "../setup/ConnectArchon.tsx";
import { archonTools, ENGINE_OF, providerReadiness } from "../setup/readiness.ts";
import { formatTime, plural } from "./format.ts";
import { listOf, ReadView, Unexpected } from "./read.tsx";

const PROVIDERS = { key: "settings.providers", command: { cmd: "providers" }, tags: ["providers"] } as const;
const AUTH = { key: "settings.auth", command: { cmd: "auth_status" }, tags: ["auth"] } as const;
const MCP = { key: "settings.mcp", command: { cmd: "mcp_servers" }, tags: ["mcp_servers"] } as const;
const MCP_REFRESH = { cmd: "mcp_servers", refresh: true };
const MCP_STATE: Readonly<Record<aether.McpServerState, { readonly tone: Tone; readonly text: string }>> = {
  ready: { tone: "success", text: "Ready" }, failed: { tone: "danger", text: "Not working" }, unknown: { tone: "neutral", text: "Not checked yet" },
};

function Headroom({ headroom }: { readonly headroom: aether.ProviderHeadroom }) {
  if (headroom.windows.length === 0) return <>Not reported</>;
  const text = headroom.windows.map((w) => `${w.name} ${Math.round(w.remaining_percent)}% left`).join(", ");
  return <>{text}{headroom.stale ? " (stale)" : ""}</>;
}

interface ProviderRowProps {
  readonly conn: ConnId;
  readonly provider: aether.ProviderInfo;
  readonly auth: aether.AuthInfo | undefined;
  readonly run: Run;
  readonly refreshAuth: () => Promise<void>;
}

function ProviderRow({ conn, provider, auth, run, refreshAuth }: ProviderRowProps) {
  invariant(provider.provider !== "", "a provider has an id");
  const readiness = providerReadiness(provider, auth);
  const source = provider.auth_source ?? auth?.source ?? null;
  const signIn = async () => {
    const engine = ENGINE_OF[provider.provider];
    invariant(engine !== undefined, "only engines sign in this way");
    if (await run.privileged({ kind: "engine.login", engine }, `A sign-in window for ${provider.name} opened. Finish there, then check again.`)) await refreshAuth();
  };
  const canSignIn = provider.engine && ENGINE_OF[provider.provider] !== undefined;
  return (
    <li className="st-item" aria-label={provider.name}>
      <div className="st-item-head">
        <h4 className="st-item-title">{provider.name}</h4>
        <Badge tone={readiness.tone}>{readiness.text}</Badge>
      </div>
      {readiness.fix === undefined ? null : <p className="st-note">{readiness.fix}</p>}
      <dl className="st-facts">
        <div><dt>Running now</dt><dd>{plural(provider.health.running, "agent")}</dd></div>
        <div><dt>Models</dt><dd>{provider.models}</dd></div>
        <div><dt>Usage left</dt><dd>{provider.headroom ? <Headroom headroom={provider.headroom} /> : "Not reported"}</dd></div>
        <div><dt>Signed in with</dt><dd>{source ?? "Nothing yet"}{auth?.subscription ? " (subscription)" : ""}</dd></div>
      </dl>
      {canSignIn ? (
        <Action conn={conn} command={{ cmd: "engine_tool" }} label={readiness.signIn ? `Sign in to ${provider.name}` : "Sign in again"}
          variant={readiness.signIn ? "primary" : "secondary"} onRun={signIn}
          confirm={{ title: `Sign in to ${provider.name}?`, body: "A sign-in window opens on the computer running Aether. Your password goes only there, never through this app.", confirmLabel: "Continue" }} />
      ) : null}
    </li>
  );
}

function ProviderList({ conn, value, auths, run, refreshAuth }: { readonly conn: ConnId; readonly value: unknown; readonly auths: unknown; readonly run: Run; readonly refreshAuth: () => Promise<void> }) {
  const providers = listOf<aether.ProviderInfo>(value);
  if (providers === null) return <Unexpected label="providers" />;
  if (providers.length === 0) return <EmptyState title="No providers">The daemon lists no model providers.</EmptyState>;
  const authList = listOf<aether.AuthInfo>(auths) ?? [];
  return (
    <ul className="st-list">
      {providers.map((p) => <ProviderRow key={p.provider} conn={conn} provider={p} auth={authList.find((a) => a.provider === p.provider)} run={run} refreshAuth={refreshAuth} />)}
    </ul>
  );
}

/**
 * Mounted for the page's life, outside the read: the repair restarts Aether and rereads the servers
 * before it ends, so it must not live inside the list those rereads replace.
 */
function ArchonRepair({ value, local, onConnected }: { readonly value: unknown; readonly local: boolean; readonly onConnected: () => void }) {
  const servers = listOf<aether.McpServerHealth>(value);
  const archon = servers === null ? null : archonTools(servers);
  const needed = archon !== null && (archon.state === "missing" || archon.state === "failed");
  const reason = archon !== null && archon.state !== "ready" ? archon.detail : "";
  if (local) return <ConnectArchon needed={needed} reason={reason} onDone={onConnected} />;
  return needed ? <p className="st-note">{reason} Connect them from the desktop app on the computer running Aether.</p> : null;
}

function McpList({ value }: { readonly value: unknown }) {
  const servers = listOf<aether.McpServerHealth>(value);
  if (servers === null) return <Unexpected label="tool servers" />;
  if (servers.length === 0) return <EmptyState title="No tool servers yet">Connect Archon's tools to give agents Archon's queue, items and runs.</EmptyState>;
  return (
    <ul className="st-list">
      {servers.map((s) => (
        <li key={s.name} className="st-item" aria-label={s.name}>
          <div className="st-item-head">
            <h4 className="st-item-title">{s.name}</h4>
            <Badge tone={MCP_STATE[s.state].tone}>{MCP_STATE[s.state].text}</Badge>
          </div>
          <p className="st-meta">{plural(s.tools?.length ?? 0, "tool")} · checked {formatTime(s.checked_at)}</p>
          {s.error ? <p className="st-note" data-state="failed">{s.error}</p> : null}
        </li>
      ))}
    </ul>
  );
}

export function ProvidersPage({ conn }: { readonly conn: ConnId }) {
  const app = useApp();
  const providers = useReread(conn, PROVIDERS.key, PROVIDERS.command, PROVIDERS.tags);
  const auths = useReread(conn, AUTH.key, AUTH.command, AUTH.tags);
  const mcp = useReread(conn, MCP.key, MCP.command, MCP.tags);
  const run = useRun(conn);
  const local = app.manager.get(conn)?.endpoint.transport.id === "ipc";
  const again = (read: typeof PROVIDERS | typeof AUTH | typeof MCP, command: Record<string, unknown> = read.command) => app.reread(conn, read.key, command, read.tags);
  const refreshUsage = async () => {
    if (await run.send({ cmd: "refresh_provider_usage" }, "Usage refreshed")) await again(PROVIDERS);
  };
  return (
    <div className="st-stack">
      <StandingNotice conn={conn} />
      <Section title="Providers" hint="Where models come from, whether each can run work now, and how much use is left.">
        <ReadView label="providers" entry={providers} onRetry={() => void again(PROVIDERS)}>
          {(value) => <ProviderList conn={conn} value={value} auths={auths?.value} run={run} refreshAuth={() => again(AUTH)} />}
        </ReadView>
        <div className="st-row">
          <Action conn={conn} command={{ cmd: "refresh_provider_usage" }} label="Refresh usage" onRun={refreshUsage} />
        </div>
        <RunNote status={run.status} />
      </Section>
      <Section title="Tool servers (MCP)" hint="Extra tools agents can use, such as Archon's.">
        <ArchonRepair value={mcp?.value} local={local} onConnected={() => void again(MCP, MCP_REFRESH)} />
        <ReadView label="tool servers" entry={mcp} onRetry={() => void again(MCP)}>
          {(value) => <McpList value={value} />}
        </ReadView>
        <div className="st-row">
          <Action conn={conn} command={MCP_REFRESH} label="Check servers again" onRun={() => again(MCP, MCP_REFRESH)} />
        </div>
      </Section>
    </div>
  );
}
