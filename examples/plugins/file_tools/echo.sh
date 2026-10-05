#!/bin/sh
# Example script tool declared from tools.json. The host passes the JSON
# arguments in PLUGIN_ARGS and returns captured stdout as the tool result.
printf 'file_tools received: %s\n' "$PLUGIN_ARGS"
