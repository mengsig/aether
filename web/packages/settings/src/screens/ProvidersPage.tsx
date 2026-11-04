// Model providers, their sign-in sources and MCP servers, all derived from core's store. Nothing
// here asks for or shows a credential: signing in opens a terminal on the host.

import { useApp, useReread } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { Badge, EmptyState, type Tone } from "@aether-ui/ui";
import { invariant } from "../kit/invariant.ts";
import { Action, RunNote, Section, StandingNotice, useRun, type Run } from "../kit/kit.tsx";
import { formatTime, plural } from "./format.ts";
import { listOf, ReadView, Unexpected } from "./read.tsx";

const PROVIDERS = { key: "settings.providers", command: { cmd: "providers" }, tags: ["providers"] } as const;
const AUTH = { key: "settings.auth", command: { cmd: "auth_status" }, tags: ["auth"] } as const;
const MCP = { key: "settings.mcp", command: { cmd: "mcp_servers" }, tags: ["mcp_servers"] } as const;
const MCP_REFRESH = { cmd: "mcp_servers", refresh: true };
/** The engine sign-in each provider id uses. */
const ENGINE_OF: Readonly<Record<string, "claude" | "codex">> = { "claude-code": "claude", codex: "codex" };

function healthText(health: aether.ProviderHealth): { readonly tone: Tone; readonly text: string } {
  if (health.backoff) return { tone: "warning", text: `Backing off: ${health.backoff.reason}` };
  if ((health.suspended_agents ?? 0) > 0) return { tone: "warning", text: plural(health.suspended_agents ?? 0, "agent") + " suspended" };
  return { tone: "success", text: health.running > 0 ? `Healthy, ${health.running} running` : "Healthy" };
}

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
  const health = healthText(provider.health);
  const source = provider.auth_source ?? auth?.source ?? null;
  const signIn = async () => {
    const engine = ENGINE_OF[provider.provider];
    invariant(engine !== undefined, "only engines sign in this way");
    if (await run.privileged({ kind: "engine.login", engine }, `Sign-in for ${provider.name} opened in a terminal on the host`)) await refreshAuth();
  };
  return (
    <li className="st-item" aria-label={provider.name}>
      <div className="st-item-head">
        <h4 className="st-item-title">{provider.name}</h4>
        <span className="st-row">
          <Badge tone={provider.configured ? "success" : "neutral"}>{provider.configured ? "Configured" : "Not configured"}</Badge>
          <Badge tone={provider.enabled ? "accent" : "neutral"}>{provider.enabled ? "Enabled" : "Disabled"}</Badge>
        </span>
      </div>
      <dl className="st-facts">
        <div><dt>Health</dt><dd><Badge tone={health.tone}>{health.text}</Badge></dd></div>
        <div><dt>Models</dt><dd>{provider.models}</dd></div>
        <div><dt>Usage headroom</dt><dd>{provider.headroom ? <Headroom headroom={provider.headroom} /> : "Not reported"}</dd></div>
        <div><dt>Sign-in source</dt><dd>{source ?? "None"}{auth?.subscription ? " (subscription)" : ""}</dd></div>
      </dl>
      {provider.engine && ENGINE_OF[provider.provider] !== undefined ? (
        <Action conn={conn} command={{ cmd: "engine_tool" }} label="Sign in" onRun={signIn}
          confirm={{ title: `Sign in to ${provider.name}?`, body: "A terminal opens on the host to finish signing in. Nothing you type there reaches this page.", confirmLabel: "Open terminal" }} />
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

function McpList({ value }: { readonly value: unknown }) {
  const servers = listOf<aether.McpServerHealth>(value);
  if (servers === null) return <Unexpected label="MCP servers" />;
  if (servers.length === 0) return <EmptyState title="No MCP servers">None are configured.</EmptyState>;
  const tone: Record<aether.McpServerState, Tone> = { ready: "success", failed: "danger", unknown: "neutral" };
  return (
    <ul className="st-list">
      {servers.map((s) => (
        <li key={s.name} className="st-item" aria-label={s.name}>
          <div className="st-item-head">
            <h4 className="st-item-title">{s.name}</h4>
            <Badge tone={tone[s.state]}>{s.state}</Badge>
          </div>
          <p className="st-meta">{s.transport} · {plural(s.tools?.length ?? 0, "tool")} · checked {formatTime(s.checked_at)}</p>
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
  const again = (read: typeof PROVIDERS | typeof AUTH | typeof MCP, command: Record<string, unknown> = read.command) => app.reread(conn, read.key, command, read.tags);
  const refreshUsage = async () => {
    if (await run.send({ cmd: "refresh_provider_usage" }, "Usage refreshed")) await again(PROVIDERS);
  };
  return (
    <div className="st-stack">
      <StandingNotice conn={conn} />
      <Section title="Providers" hint="Where models come from, whether they can be used now, and how much room is left.">
        <ReadView label="providers" entry={providers} onRetry={() => void again(PROVIDERS)}>
          {(value) => <ProviderList conn={conn} value={value} auths={auths?.value} run={run} refreshAuth={() => again(AUTH)} />}
        </ReadView>
        <div className="st-row">
          <Action conn={conn} command={{ cmd: "refresh_provider_usage" }} label="Refresh usage" onRun={refreshUsage} />
        </div>
        <RunNote status={run.status} />
      </Section>
      <Section title="MCP servers" hint="Tool servers the daemon can reach.">
        <ReadView label="MCP servers" entry={mcp} onRetry={() => void again(MCP)}>{(value) => <McpList value={value} />}</ReadView>
        <div className="st-row">
          <Action conn={conn} command={MCP_REFRESH} label="Check servers again" onRun={() => again(MCP, MCP_REFRESH)} />
        </div>
      </Section>
    </div>
  );
}
