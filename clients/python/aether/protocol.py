"""The Aether daemon protocol as Python types and command builders.

Generated from clients/protocol.schema.json by scripts/generate_clients.py.
Do not edit: change the Rust types and regenerate.
"""

from __future__ import annotations

from typing import Any, Literal, TypedDict

PROTOCOL_VERSION = 19
MIN_PROTOCOL_VERSION = 17

from ._protocol_types import *

from ._protocol_commands.add import (
    add_approval_rule,
)

from ._protocol_commands.alias import (
    alias_repository,
)

from ._protocol_commands.archive import (
    archive,
)

from ._protocol_commands.auth import (
    auth_status,
)

from ._protocol_commands.cancel import (
    cancel,
    cancel_pairing,
)

from ._protocol_commands.compact import (
    compact,
)

from ._protocol_commands.complete import (
    complete_path,
)

from ._protocol_commands.configure import (
    configure_github_watcher,
    configure_remote,
)

from ._protocol_commands.confirm import (
    confirm_pairing,
)

from ._protocol_commands.create import (
    create_agent,
    create_memory,
    create_schedule,
    create_task,
)

from ._protocol_commands.engine import (
    engine_tool,
    engine_tools,
)

from ._protocol_commands.export import (
    export_run,
)

from ._protocol_commands.forget import (
    forget_memory,
)

from ._protocol_commands.get import (
    get_agent,
    get_artifact,
    get_attention,
    get_checkpoint,
    get_events,
    get_git_summary,
    get_github_watcher,
    get_inbox,
    get_invocation,
    get_pairing,
    get_push_config,
    get_remote,
    get_run_report,
    get_schedule,
    get_snapshot,
    get_task,
    get_transcript,
    get_tree,
)

from ._protocol_commands.hello import (
    hello,
)

from ._protocol_commands.import_ import (
    import_run,
)

from ._protocol_commands.interrupt import (
    interrupt,
)

from ._protocol_commands.latest import (
    latest_checkpoint,
)

from ._protocol_commands.list import (
    list_agents,
    list_approval_rules,
    list_approvals,
    list_artifacts,
    list_devices,
    list_invocations,
    list_models,
    list_repository_aliases,
    list_runs,
    list_schedules,
    list_tasks,
    list_workspaces,
)

from ._protocol_commands.mcp import (
    mcp_servers,
)

from ._protocol_commands.memories import (
    memories,
)

from ._protocol_commands.pause import (
    pause,
)

from ._protocol_commands.pin import (
    pin_memory,
)

from ._protocol_commands.ping import (
    ping,
)

from ._protocol_commands.preview import (
    preview_retention,
)

from ._protocol_commands.providers import (
    providers,
)

from ._protocol_commands.publish import (
    publish_artifact,
)

from ._protocol_commands.read import (
    read_artifact,
)

from ._protocol_commands.replay import (
    replay,
)

from ._protocol_commands.reply import (
    reply_to_question,
)

from ._protocol_commands.resolve import (
    resolve_approval,
    resolve_submission,
)

from ._protocol_commands.restart import (
    restart,
)

from ._protocol_commands.restore import (
    restore_memory,
)

from ._protocol_commands.resume import (
    resume,
)

from ._protocol_commands.revoke import (
    revoke_approval_rule,
    revoke_device,
)

from ._protocol_commands.schedule import (
    schedule_history,
)

from ._protocol_commands.send import (
    send_message,
)

from ._protocol_commands.set import (
    set_approvals,
)

from ._protocol_commands.shutdown import (
    shutdown,
)

from ._protocol_commands.start import (
    start_pairing,
)

from ._protocol_commands.status import (
    status,
)

from ._protocol_commands.subscribe import (
    subscribe,
    subscribe_push,
)

from ._protocol_commands.sync import (
    sync_repository_knowledge,
)

from ._protocol_commands.test import (
    test_push,
)

from ._protocol_commands.trigger import (
    trigger,
)

from ._protocol_commands.unsubscribe import (
    unsubscribe,
    unsubscribe_push,
)

from ._protocol_commands.update import (
    update_agent,
    update_device,
    update_schedule,
)

from ._protocol_commands.validate import (
    validate_workspace,
)

from ._protocol_commands.wake import (
    wake,
)
