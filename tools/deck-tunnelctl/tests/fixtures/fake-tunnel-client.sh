#!/bin/sh
# Test-only argv recorder. Production code never invokes a shell.
for argument in "$@"; do
  printf '%s\n' "$argument" >> "$DECK_TUNNELCTL_FAKE_LOG"
done
if [ "$1" = "runtimes" ] && [ "$2" = "status" ]; then
  printf '%s\n' '{"process_running":true,"healthy":true,"ready":true,"stale":false,"tunnel_id":"tunnel_fixture","health_url_file":"/fixture/health.url"}'
  exit 0
fi
if [ "$1" = "health" ]; then
  printf '%s\n' '{"healthz":{"ok":true},"readyz":{"ok":true},"control_plane_poll":{"ok":true,"value":1}}'
  exit 0
fi
if [ "$1" = "runtimes" ] && [ "$2" = "connect" ]; then
  printf '%s\n' '{"ok":true}'
  exit 0
fi
if [ "$1" = "runtimes" ] && [ "$2" = "stop" ]; then
  printf '%s\n' '{"ok":true}'
  exit 0
fi
if [ "$1" = "runtimes" ] && [ "$2" = "rm" ]; then
  printf '%s\n' '{"ok":true}'
  exit 0
fi
exit 64
