"""Live ordinary-language timers, plus durable pause/restart/resume validation."""
import argparse
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import time

parser = argparse.ArgumentParser()
parser.add_argument('--binary', required=True)
parser.add_argument('--output', required=True)
parser.add_argument('--model', action='append', required=True)
args = parser.parse_args()
from aether_client import AetherClient, init_git_workspace, isolated_env, start_daemon, stop_daemon, protocol

binary = str(Path(args.binary).resolve())
report = {'binary_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(), 'results': []}
output = Path(args.output)
output.parent.mkdir(parents=True, exist_ok=True)
for model in args.model:
    result = {'model': model, 'passed': False}
    report['results'].append(result)
    with tempfile.TemporaryDirectory(prefix='aether-schedule-live-') as directory:
        env = isolated_env(directory)
        workspace = Path(directory) / 'workspace'
        workspace.mkdir()
        (workspace / 'README.md').write_text('# Timer documentation fixture\n\nA small read-only scheduling check.\n')
        init_git_workspace(workspace)
        try:
            start_daemon(binary, env, str(workspace))
            with AetherClient(env['AETHER_SOCKET']) as client:
                prompt = 'Every minute, read README.md and tell me its title. Stop the timer after two deliveries.'
                result['prompt'] = prompt
                root = client.create_agent(model=model, effort='low', objective=prompt)
                result['agent'] = root['id']
                deadline = time.monotonic() + 420
                paused = None
                while time.monotonic() < deadline:
                    jobs = client.request(protocol.list_schedules(target=root['id']))['data']
                    if len(jobs) > 1:
                        raise AssertionError(f'Created {len(jobs)} timers instead of one')
                    current = client.get_agent(root['id'])
                    if current['status']['state'] in ('failed', 'cancelled', 'completed'):
                        raise AssertionError(f'Agent ended unexpectedly: {current["status"]}')
                    if jobs and current['status']['state'] == 'waiting':
                        paused = client.request(protocol.update_schedule(id=jobs[0]['id'], action='pause'))['data']
                        break
                    time.sleep(0.25)
                if paused is None or current['status']['state'] != 'waiting':
                    raise TimeoutError('No waiting agent with a timer before the deadline')
                assert paused['recurrence'] == {'kind': 'interval', 'seconds': 60}, paused
                assert paused['max_deliveries'] == 2, paused
                result['paused_before_restart'] = paused
            stop_daemon(binary, env)
            start_daemon(binary, env, str(workspace))
            with AetherClient(env['AETHER_SOCKET']) as client:
                restored = client.request(protocol.get_schedule(id=paused['id']))['data']
                assert restored == paused, (restored, paused)
                client.request(protocol.update_schedule(id=paused['id'], action='resume'))
                deadline = time.monotonic() + 420
                while time.monotonic() < deadline:
                    job = client.request(protocol.get_schedule(id=paused['id']))['data']
                    current = client.get_agent(root['id'])
                    if current['status']['state'] in ('failed', 'cancelled', 'completed'):
                        raise AssertionError(f'Agent ended unexpectedly: {current["status"]}')
                    if job['status']['state'] == 'exhausted' and current['status']['state'] == 'waiting' and current['inbox_cursor'] >= job['last_inbox_seq']:
                        break
                    time.sleep(0.5)
                else:
                    raise TimeoutError('Timer did not finish two delivered tasks')
                final_jobs = client.request(protocol.list_schedules(target=root['id']))['data']
                assert len(final_jobs) == 1, final_jobs
                history = client.request(protocol.schedule_history(id=job['id'], limit=100))['data']
                deliveries = [item for item in history if item['outcome']['outcome'] == 'delivered']
                assert len(deliveries) == job['deliveries'] == 2, history
                events = client.request(protocol.get_events(after=0, limit=10000, query={'agent': root['id']}))['data']
                if isinstance(events, dict):
                    events = events['events']
                failures = [event['kind'] for event in events if event['kind']['type'] == 'tool.failed' or (event['kind']['type'] == 'tool.succeeded' and event['kind']['outcome'].get('is_error', False))]
                assert not failures, failures
                first = min(item['recorded_at'] for item in deliveries)
                messages = [event['kind'] for event in events if event['kind']['type'] == 'agent.message' and event['kind']['role'] == 'assistant' and event['at'] >= first]
                replies = [''.join(block.get('text', '') for block in message['content']) for message in messages]
                assert sum('Timer documentation fixture' in reply for reply in replies) >= 2, replies
                result.update(passed=True, schedule=job, history=history, replies=replies, restart_preserved_timer=True)
        except Exception as error:
            result['error'] = f'{type(error).__name__}: {error}'
        finally:
            stop_daemon(binary, env)
            output.write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps({'model': model, 'passed': result['passed'], 'error': result.get('error')}), flush=True)
sys.exit(0 if all(result['passed'] for result in report['results']) else 1)
