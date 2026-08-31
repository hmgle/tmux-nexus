#!/usr/bin/env bash
# tmux-nexus plugin entry point.

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/utils.sh
source "$CURRENT_DIR/scripts/utils.sh"

search_command="$(tnx_shell_quote "$CURRENT_DIR/tnx") search"
prompt_search_command="$search_command --query-option @tmux_nexus_last_query_#{client_pid} --require-query"
pane_search_command="$search_command --scope pane"
pane_prompt_search_command="$pane_search_command --query-option @tmux_nexus_last_query_#{client_pid} --require-query"
motion_command="$(tnx_shell_quote "$CURRENT_DIR/scripts/motion-s.sh") #{client_pid} #{window_id} #{q:client_name}"
motion2_command="$(tnx_shell_quote "$CURRENT_DIR/scripts/motion-s2.sh") #{client_pid} #{window_id} #{q:client_name}"
manager_command="$(tnx_shell_quote "$CURRENT_DIR/tnx") manage"

launch_key=$(tmux show-option -gqv "@tmux_nexus_launch_key" 2>/dev/null)
launch_key="${TNX_LAUNCH_KEY:-${launch_key:-g}}"
pane_key=$(tmux show-option -gqv "@tmux_nexus_pane_key" 2>/dev/null)
pane_key="${TNX_PANE_KEY:-$pane_key}"
motion_key=$(tmux show-option -gqv "@tmux_nexus_motion_key" 2>/dev/null)
motion_key="${TNX_MOTION_KEY:-${motion_key:-s}}"
motion2_key=$(tmux show-option -gqv "@tmux_nexus_motion2_key" 2>/dev/null)
motion2_key="${TNX_MOTION2_KEY:-${motion2_key:-}}"
if [ "${TNX_MANAGER_KEY+x}" = x ]; then
    manager_key="$TNX_MANAGER_KEY"
elif tmux show-option -g "@tmux_nexus_manager_key" >/dev/null 2>&1; then
    manager_key=$(tmux show-option -gqv "@tmux_nexus_manager_key" 2>/dev/null)
elif [ "${TMUX_FZF_LAUNCH_KEY+x}" = x ]; then
    manager_key="$TMUX_FZF_LAUNCH_KEY"
else
    manager_key="F"
fi
motion_copy_mode_no_prefix=$(tmux show-option -gqv "@tmux_nexus_motion_copy_mode_no_prefix" 2>/dev/null)
motion_copy_mode_no_prefix="${motion_copy_mode_no_prefix:-${TNX_MOTION_COPY_MODE_NO_PREFIX:-0}}"
prompt_query=$(tmux show-option -gqv "@tmux_nexus_prompt_query" 2>/dev/null)
prompt_query="${prompt_query:-${TNX_PROMPT_QUERY:-0}}"

case "$prompt_query" in
    1|true|yes|on)
        tmux bind-key "$launch_key" command-prompt -F -p "history search:" -T search \
            "set-option -gq @tmux_nexus_last_query_#{client_pid} '%%%'" \
            \; run-shell -b "$prompt_search_command"
        ;;
    *)
        tmux bind-key "$launch_key" run-shell -b "$search_command"
        ;;
esac

if [ -n "$pane_key" ]; then
    case "$prompt_query" in
        1|true|yes|on)
            tmux bind-key "$pane_key" command-prompt -F -p "pane history search:" -T search \
                "set-option -gq @tmux_nexus_last_query_#{client_pid} '%%%'" \
                \; run-shell -b "$pane_prompt_search_command"
            ;;
        *)
            tmux bind-key "$pane_key" run-shell -b "$pane_search_command"
            ;;
    esac
fi

if [ -n "$motion_key" ]; then
    tmux bind-key "$motion_key" run-shell "$motion_command"
fi

if [ -n "$motion2_key" ]; then
    tmux bind-key "$motion2_key" run-shell "$motion2_command"
fi

if [ -n "$manager_key" ]; then
    tmux bind-key "$manager_key" run-shell -b "$manager_command"
fi

case "$motion_copy_mode_no_prefix" in
    1|true|yes|on)
        mode_keys=$(tmux show-option -gqv mode-keys 2>/dev/null)
        if [ "$mode_keys" = "vi" ]; then
            copy_mode_table="copy-mode-vi"
        else
            copy_mode_table="copy-mode"
        fi
        if [ -n "$motion_key" ]; then
            tmux bind-key -T "$copy_mode_table" "$motion_key" run-shell "$motion_command"
        fi
        if [ -n "$motion2_key" ]; then
            tmux bind-key -T "$copy_mode_table" "$motion2_key" run-shell "$motion2_command"
        fi
        ;;
esac

if [ -z "$(tmux show-option -gqv "@tnx_loaded")" ]; then
    tmux set-option -g "@tnx_loaded" "1"
    loaded_message="tmux-nexus loaded: Prefix+$launch_key search"
    if [ -n "$manager_key" ]; then
        loaded_message="$loaded_message, Prefix+$manager_key manage"
    fi
    if [ -n "$motion_key" ]; then
        loaded_message="$loaded_message, Prefix+$motion_key motion"
    fi
    tmux display-message "$loaded_message" 2>/dev/null || :
fi
