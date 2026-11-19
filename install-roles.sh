#!/bin/sh
# Registers the two Maestri roles from docs/. Run inside a Maestri terminal.
# Picks the role files for the configured language ("ja" or "en", default "en").
set -eu
dir=$(cd "$(dirname "$0")" && pwd -P)

# Same lookup order as the decidle binary: $DECIDLE_CONFIG, then ~/.config/decidle/config.json.
config=${DECIDLE_CONFIG:-$HOME/.config/decidle/config.json}
lang=en
if [ -f "$config" ]; then
  found=$(tr -d '\n' < "$config" | sed -n 's/.*"language"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
  if [ -n "$found" ]; then
    lang=$found
  fi
fi
case "$lang" in
  ja | en) ;;
  *)
    echo "install-roles.sh: unknown language \"$lang\" in $config (use \"ja\" or \"en\")" >&2
    exit 1
    ;;
esac

register() {
  name=$1
  file=$2
  if maestri role show "$name" >/dev/null 2>&1; then
    maestri role write "$name" "$(cat "$file")"
  else
    maestri role create "$name" "$(cat "$file")" --scope global
  fi
}

register "Report Reviewer" "$dir/docs/role-reviewer.$lang.md"
register "Report Orchestrator" "$dir/docs/role-orchestrator.$lang.md"
