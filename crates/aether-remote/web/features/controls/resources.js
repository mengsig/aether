import { el } from '../../ui/dom.js';
import { panel, card, action, details, readList, replaceRows, display } from './ui.js';
import { required, replyData, record, artifactBytes } from './protocol.js';
import { count, dateLabel, fileSize, words } from './format.js';
import { rawDetails, usageSummary, runTitle, runReport } from './presentation.js';

/** @typedef {import('./ui.js').Panel} Panel */

/** @param {Panel} view @param {Record<string,unknown>} artifact @param {string[]} urls */
function artifactRow(view, artifact, urls) {
  const id = required(artifact.id, 'Artifact ID');
  const root = card(required(artifact.name, 'Artifact name'), display(artifact.summary));
  const content = el('div', 'stack');
  const actions = el('div', 'row controls-actions');
  root.append(details({ Format: display(artifact.content_type), Size: fileSize(artifact.size_bytes), Created: dateLabel(artifact.created_at) }));
  if (artifact.evicted_at) root.append(el('p', 'muted', `Content removed ${dateLabel(artifact.evicted_at)}. Metadata remains available.`));
  root.append(rawDetails(artifact));
  actions.append(action(view, 'View file details', async () => {
    const reply = await view.context.transport.request({ cmd: 'get_artifact', id });
    const data = record(replyData(reply, 'artifact'));
    if (view.alive()) { const disclosure = rawDetails(data); disclosure.open = true; content.replaceChildren(disclosure); }
  }));
  actions.append(action(view, 'Preview file', async () => {
    const reply = await view.context.transport.request({ cmd: 'read_artifact', id, max_bytes: 262144 });
    const result = artifactBytes(replyData(reply, 'artifact_bytes'));
    if (!view.alive()) return;
    content.replaceChildren(el('p', 'muted', result.truncated ? 'Partial content: limited to 256 KiB.' : `${fileSize(result.bytes.length)} received.`));
    const mime = String(result.artifact.content_type || 'application/octet-stream');
    if (/^text\/|^application\/(json|xml)/.test(mime)) {
      content.append(el('pre', 'controls-preview', new TextDecoder().decode(result.bytes)));
    } else content.append(el('p', 'muted', 'Binary artifact. Download the received bytes to inspect it.'));
    const url = URL.createObjectURL(new Blob([result.bytes], { type: 'application/octet-stream' }));
    urls.push(url);
    const download = el('a', 'button', result.truncated ? 'Download partial bytes' : 'Download artifact');
    download.href = url;
    download.download = String(artifact.name).replace(/[\\/]/g, '_') + (result.truncated ? '.partial' : '');
    content.append(download);
  }));
  root.append(actions, content);
  return root;
}

/** @param {HTMLElement} container @param {import('../../types.js').Context} context @param {string} agent */
export function mountArtifacts(container, context, agent) {
  required(agent, 'Agent ID');
  const view = panel(container, context);
  const list = el('div', 'stack');
  /** @type {string[]} */
  const urls = [];
  const refresh = async () => {
    const artifacts = await readList(view, { cmd: 'list_artifacts', query: { agent, limit: 100 } }, 'artifacts');
    if (!view.alive()) return;
    urls.splice(0).forEach(url => URL.revokeObjectURL(url));
    replaceRows(list, artifacts.map(artifact => artifactRow(view, artifact, urls)), 'No artifacts published by this agent.');
  };
  view.body.append(card('Artifacts', 'Files shared by this agent. Preview or download up to 256 KiB at a time.'), action(view, 'Refresh artifacts', refresh), list);
  void view.run(refresh);
  return () => { view.dispose(); urls.forEach(url => URL.revokeObjectURL(url)); };
}

/** @param {Panel} view @param {Record<string,unknown>} run */
function runRow(view, run) {
  const id = required(run.run, 'Run ID');
  const root = card(runTitle(run, view.context.store.get().agents));
  const report = el('div', 'stack');
  root.append(details({ Started: dateLabel(run.started_at), Updated: dateLabel(run.updated_at),
    Agents: count(run.agents), 'Live agents': count(run.agents_live) }), usageSummary(run.usage), rawDetails(run));
  root.append(action(view, 'Read run report', async () => {
    const reply = await view.context.transport.request({ cmd: 'get_run_report', run: id });
    const result = record(replyData(reply, 'run_report'));
    if (!view.alive()) return;
    report.replaceChildren(runReport(result));
  }), report);
  return root;
}

/** @param {HTMLElement} container @param {import('../../types.js').Context} context */
export function mountRuns(container, context) {
  const view = panel(container, context);
  const list = el('div', 'stack');
  const refresh = async () => {
    const runs = await readList(view, { cmd: 'list_runs', limit: 30 }, 'runs');
    if (view.alive()) replaceRows(list, runs.map(run => runRow(view, run)), 'No stored runs.');
  };
  view.body.append(card('Runs and reports', 'The most recent 30 runs across this daemon. Missing usage stays unknown.'),
    action(view, 'Refresh runs', refresh), list);
  void view.run(refresh);
  return view.dispose;
}

/** @param {HTMLElement} container @param {import('../../types.js').Context} context */
export function mountDiagnostics(container, context) {
  const view = panel(container, context);
  const providerList = el('div', 'stack');
  const mcpList = el('div', 'stack');
  const refresh = async () => {
    const providers = await readList(view, { cmd: 'providers' }, 'providers');
    if (!view.alive()) return;
    replaceRows(providerList, providers.map(providerRow), 'No providers reported.');
    const servers = await readList(view, { cmd: 'mcp_servers', refresh: false }, 'mcp_servers');
    if (view.alive()) replaceRows(mcpList, servers.map(serverRow), 'No MCP servers configured.');
  };
  const providers = card('Provider health');
  providers.append(providerList);
  const mcp = card('MCP servers', 'Health and tool information from the current connections.');
  mcp.append(mcpList, action(view, 'Reconnect MCP servers', async () => {
    const reply = await view.change({ cmd: 'mcp_servers', refresh: true }, 'Reconnect all configured MCP servers and rediscover their tools?');
    if (reply) await refresh();
  }, true));
  const signIn = card('Provider sign-in', 'Sign-in and credential changes are managed on the Aether host. Your phone never receives provider credentials.');
  const commands = el('details', 'controls-record');
  commands.append(el('summary', '', 'Host sign-in instructions'), el('pre', 'controls-preview', 'aether login PROVIDER\naether logout PROVIDER'));
  signIn.append(commands);
  view.body.append(action(view, 'Refresh diagnostics', refresh), providers, mcp, signIn);
  view.sync();
  void view.run(refresh);
  return view.dispose;
}

/** @param {Record<string,unknown>} provider */
function providerRow(provider) {
  record(provider);
  const health = provider.health == null ? {} : record(provider.health);
  const root = el('section', 'stack controls-resource');
  root.append(el('h3', '', required(provider.name ?? provider.provider, 'Provider name')),
    details({ Setup: provider.configured === true ? 'Configured' : provider.configured === false ? 'Not configured' : 'Not reported',
      Availability: provider.enabled === true ? 'Enabled' : provider.enabled === false ? 'Disabled' : 'Not reported',
      'Active turns': count(health.running), Models: count(provider.models) }));
  if (health.backoff) root.append(el('p', 'controls-error', 'This provider is holding back new turns. Inspect its record for the reason and retry time.'));
  if (Array.isArray(health.recent_errors) && health.recent_errors.length) root.append(el('p', 'controls-error', 'Recent provider errors reported. Inspect its record for details.'));
  root.append(rawDetails(provider));
  return root;
}

/** @param {Record<string,unknown>} server */
function serverRow(server) {
  record(server);
  const root = el('section', 'stack controls-resource');
  root.append(el('h3', '', required(server.name, 'Server name')), details({ Status: words(server.state),
    'Available tools': Array.isArray(server.tools) ? count(server.tools.length) : 'Not reported' }));
  if (typeof server.error === 'string') root.append(el('p', 'controls-error', server.error));
  root.append(rawDetails(server));
  return root;
}
