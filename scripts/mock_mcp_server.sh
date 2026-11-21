#!/usr/bin/env bash
# A minimal MCP server over stdio, for tests and for trying the MCP client by
# hand. Speaks JSON-RPC on stdin/stdout; nothing else may go to stdout.
#
#   AETHER_MOCK_MCP_DIE=1     exit after tools/list, so a caller can prove that
#                             a server which dies is reported and never hangs.
#   AETHER_MOCK_MCP_STRICT=1  refuse any tools/call carrying `_meta`, like a
#                             server that rejects params it does not know.
#   AETHER_MOCK_MCP_CALLS=<file>      append each tools/call's tool name to it.
#   AETHER_MOCK_MCP_DROP_ONCE=<file>  on the first tools/call (while <file> is
#                             absent) create <file> and exit without replying,
#                             like a server whose connection drops mid-call.
set -u

emit() { printf '%s\n' "$1"; }

# The id of the request being answered. Requests here are one per line.
request_id() { printf '%s' "$1" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'; }

tools_json='{"tools":[
  {"name":"read_note","description":"Read the note.","inputSchema":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}},
  {"name":"write_note","description":"Write the note.","inputSchema":{"type":"object","properties":{"key":{"type":"string"},"text":{"type":"string"}},"required":["key","text"]}},
  {"name":"read_env","description":"Report one variable of this server own environment (test only).","inputSchema":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}},
  {"name":"echo_request","description":"Return the raw tools/call request as text (test only).","inputSchema":{"type":"object"}},
  {"name":"post_once","description":"Post a message; a repeat under the same idempotency_key is the first post.","inputSchema":{"type":"object","properties":{"text":{"type":"string"},"idempotency_key":{"type":"string"}},"required":["text"]}}
]}'

while IFS= read -r line; do
  id=$(request_id "$line")
  case "$line" in
    *'"method":"initialize"'*)
      emit "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"mock\",\"version\":\"1\"}}}"
      ;;
    *'"method":"notifications/initialized"'*)
      : # a notification takes no reply
      ;;
    *'"method":"tools/list"'*)
      emit "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":$(printf '%s' "$tools_json" | tr -d '\n')}"
      if [ "${AETHER_MOCK_MCP_DIE:-0}" = "1" ]; then exit 0; fi
      ;;
    *'"method":"tools/call"'*)
      if [ "${AETHER_MOCK_MCP_STRICT:-0}" = "1" ] && [[ "$line" == *'"_meta"'* ]]; then
        emit "{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32602,\"message\":\"unknown param _meta\"}}"
        continue
      fi
      name=$(printf '%s' "$line" | sed -n 's/.*"name":"\([a-z_]*\)".*/\1/p')
      if [ -n "${AETHER_MOCK_MCP_CALLS:-}" ]; then printf '%s\n' "$name" >>"$AETHER_MOCK_MCP_CALLS"; fi
      if [ -n "${AETHER_MOCK_MCP_DROP_ONCE:-}" ] && [ ! -e "$AETHER_MOCK_MCP_DROP_ONCE" ]; then
        : >"$AETHER_MOCK_MCP_DROP_ONCE"
        exit 0
      fi
      case "$name" in
        read_note)  emit "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"note contents\"}]}}" ;;
        write_note) emit "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"written\"}]}}" ;;
        post_once)  emit "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"posted\"}]}}" ;;
        read_env)
          var=$(printf '%s' "$line" | sed -n 's/.*"key":"\([A-Za-z_][A-Za-z0-9_]*\)".*/\1/p')
          value=$(printenv "$var" || echo UNSET)
          emit "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"$value\"}]}}"
          ;;
        echo_request)
          escaped=$(printf '%s' "$line" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')
          emit "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"$escaped\"}]}}"
          ;;
        *)          emit "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"no such tool\"}],\"isError\":true}}" ;;
      esac
      ;;
    *)
      [ -n "$id" ] && emit "{\"jsonrpc\":\"2.0\",\"id\":$id,\"error\":{\"code\":-32601,\"message\":\"unknown method\"}}"
      ;;
  esac
done
