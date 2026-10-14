#!/usr/bin/env bash
# Prompt for the second motion character after motion-s2.sh stores the first.

set -o pipefail

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/utils.sh
source "$CURRENT_DIR/scripts/utils.sh"
client_pid="${1:-}"
target_window="${2:-}"
target_client="${3:-}"

if [ -z "$client_pid" ] || [ -z "$target_window" ]; then
    exit 0
fi

query_option="@tmux_nexus_motion_query_$client_pid"
second_option="@tmux_nexus_motion_query_second_$client_pid"
first=$(tmux show-option -gqv "$query_option" 2>/dev/null)

if [ -z "$first" ]; then
    exit 0
fi

submit_command="$(tnx_shell_quote "$CURRENT_DIR/scripts/motion-s2-submit.sh") $(tnx_shell_quote "$client_pid") $(tnx_shell_quote "$target_window")"
[ -z "$target_client" ] || submit_command="$submit_command $(tnx_shell_quote "$target_client")"

# Leave the client command queue free to send keys to the hint window.
tmux command-prompt -1F -p "motion 2/2: $first" \
    "set-option -gq $second_option '%%%'" \
    \; run-shell -b "$submit_command"
