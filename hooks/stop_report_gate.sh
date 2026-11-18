#!/bin/sh
# Stop hook: checks the agent's report JSON and renders the report page.
# Also usable as `stop_report_gate.sh render <report.json>`.
# Does nothing on a machine where report-gate has not been installed.
bin="$HOME/.local/bin/report-gate"
[ -x "$bin" ] || exit 0
[ $# -gt 0 ] || set -- stop
exec "$bin" "$@"
