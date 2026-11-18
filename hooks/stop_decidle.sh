#!/bin/sh
# Runs the decidle binary by hand: `stop_decidle.sh render <report.json>` writes a report page.
# Not used from ~/.claude/settings.json: the hooks there call ~/.local/bin/decidle directly
# (see docs/settings-snippet.json). With no arguments it runs `decidle stop`.
# Does nothing on a machine where decidle has not been installed.
bin="$HOME/.local/bin/decidle"
[ -x "$bin" ] || exit 0
[ $# -gt 0 ] || set -- stop
exec "$bin" "$@"
