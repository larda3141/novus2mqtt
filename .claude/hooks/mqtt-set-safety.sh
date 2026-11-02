#!/usr/bin/env bash
# PreToolUse hook: blocks Bash mosquitto_pub commands that target /set topics
# on the live MQTT broker, since those physically change the HVAC unit.
#
# Wired in .claude/settings.json under hooks.PreToolUse, matcher "Bash".
#
# Override mechanism: append `# CONFIRMED:<short reason>` to the command after
# explicit user approval. Example:
#   mosquitto_pub -h <broker> ... -t novus2mqtt/.../bypass/set -m 'on'  # CONFIRMED: user asked to open bypass
#
# Exit codes:
#   0 — allow
#   2 — block, show stderr to the model

set -euo pipefail

cmd=$(python3 -c 'import sys, json; print(json.load(sys.stdin).get("tool_input", {}).get("command", ""))')

# Only inspect mosquitto_pub commands targeting a /set topic.
if [[ "$cmd" != *"mosquitto_pub"* ]] || [[ "$cmd" != *"/set"* ]]; then
  exit 0
fi

# Explicit override marker — only after user has confirmed in chat.
if [[ "$cmd" == *"# CONFIRMED:"* ]]; then
  exit 0
fi

cat >&2 <<EOF
BLOCKED: live HVAC write attempt.

Command: $cmd

This mosquitto_pub targets a /set topic on the production MQTT broker, which
controls the actual Paul Novus unit. Publishing here will physically change
fan speed, bypass, mode, or another live setting.

To proceed, you must:
  1. Get explicit user confirmation (do NOT assume), then
  2. Re-issue the command with a CONFIRMED marker, e.g.:
       mosquitto_pub ... -m 'value'  # CONFIRMED: <one-line reason>

If the user only wants to *read* a topic, use mosquitto_sub instead.
EOF
exit 2
