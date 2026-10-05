#!/bin/sh
# Example script tool. The host passes the JSON arguments in PLUGIN_ARGS and
# returns captured stdout as the tool result.
printf 'hooks_demo received: %s\n' "$PLUGIN_ARGS"
