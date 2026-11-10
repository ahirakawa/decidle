#!/bin/sh
# Registers the two Maestri roles from docs/. Run inside a Maestri terminal.
set -eu
dir=$(cd "$(dirname "$0")" && pwd -P)

register() {
  name=$1
  file=$2
  if maestri role show "$name" >/dev/null 2>&1; then
    maestri role write "$name" "$(cat "$file")"
  else
    maestri role create "$name" "$(cat "$file")" --scope global
  fi
}

register "Report Reviewer" "$dir/docs/role-reviewer.md"
register "Report Orchestrator" "$dir/docs/role-orchestrator.md"
