#!/usr/bin/env bash
# Prompt for one character and launch tmux-nexus motion mode.

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/utils.sh
source "$CURRENT_DIR/scripts/utils.sh"

client_pid="${1:-}"
target_window="${2:-}"
target_client="${3:-}"
[ -n "$client_pid" ] && [ -n "$target_window" ] || exit 0

query_option="@tmux_nexus_motion_query_$client_pid"
run_command="$(tnx_shell_quote "$CURRENT_DIR/scripts/motion-run.sh") s $(tnx_shell_quote "$client_pid") $(tnx_shell_quote "$target_window")"
[ -z "$target_client" ] || run_command="$run_command $(tnx_shell_quote "$target_client")"

# Leave the client command queue free to send keys to the hint window.
tmux command-prompt -1F -p "motion:" \
    "set-option -gq $query_option '%%%'" \
    \; run-shell -b "$run_command"
