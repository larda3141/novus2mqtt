---
description: Capture a bus dump from the running novus2mqtt instance for the given duration
argument-hint: "<seconds>"
allowed-tools: Bash(systemctl is-active:*), Read(//tmp/**)
---

Capture $ARGUMENTS seconds of bus traffic into `/tmp/bus_dump_$(date +%s).log`.

Steps:
1. Verify novus2mqtt is running (`systemctl is-active novus2mqtt` or check the lockfile at `/tmp/novus2mqtt.lock`).
2. Confirm `BUS_DUMP_FILE` is set in `.env` so the binary is actually writing dumps.
3. Note the current dump file size, wait $ARGUMENTS seconds, then report:
   - bytes captured
   - frame count (lines)
   - breakdown by command code (column 5 in the log)

Do not parse payload semantics here — for analysis, hand the dump to the `log-analyzer` subagent or load the `bus-reverse-engineer` skill.
