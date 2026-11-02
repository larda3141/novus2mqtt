# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

novus2mqtt — Rust async MQTT bridge for Paul Novus 300/450 heat recovery ventilation units. Communicates via RS485 serial bus (9600/8/Mark/1) and publishes bidirectional state to MQTT for Home Assistant integration.

## Build & Test Commands

```bash
cargo build                      # Debug build
cargo build --release            # Size-optimized release (LTO, strip, panic=abort)
cargo test                       # Run all tests
cargo test <test_name>           # Run a single test
cargo fmt --check                # Check formatting
cargo clippy -- -D warnings      # Lint (CI treats warnings as errors)
cargo run -- validate-config     # Validate device.yaml structure
cargo run -- health-check        # Check MQTT and serial connectivity
```

CI runs `fmt --check` → `clippy -D warnings` → `build` → `test` → `cargo audit`.

## Architecture

**Data flow — receive**: Serial bytes → `serial.rs` (async tokio-serial read) → `protocol.rs` (frame validation, CRC16) → `app.rs:process_bus_message` → `payload.rs` (raw bytes → JSON) → `mqtt.rs` (publish to broker)

**Data flow — send**: MQTT `/set` command → `app.rs:handle_set_command` → `payload.rs:encode_command_payload` → write queue → `protocol.rs:build_bus_message` → `serial.rs:write_data`. Writes are executed after receiving a 0x84 (ping) or 0x87 (checksum) to slot into the bus polling cycle.

**Key modules** (`src/`):
- `app/` — Main orchestrator (`NovusConnector`), split across files:
  - `app/mod.rs` — `NovusConnector` struct + initialize/run loop + the parts not yet split (~1100 lines).
  - `app/outcome.rs` — Pure throttling decision logic (`MqttOutcome`, `ThrottleState`, `decide_mqtt_outcome`) + unit tests.
  - `app/write_queue.rs` — `WriteItem`, `execute_pending_write`, `handle_set_command`, command-topic resolution.
  - `app/diagnostics.rs` — Periodic diagnostics publish, HA bridge-discovery, CPU/IP helpers.
  - `app/tui.rs` — Keyboard listener thread, key-command dispatch, activity-line emission with pause/blackout modes, terminal raw-mode helpers.
  - `app/registers.rs` — Per-address metadata (small; placeholder for the future zero-alloc register lookup).
- `protocol.rs` — Bus frame constants, custom CRC16-CCITT (init=0, NOT standard 0xFFFF), message builder. Checksum is at bytes 4-5, computed over bytes [0:4]+[6:].
- `serial.rs` — Async serial I/O with state-machine parser. Mark parity set via raw termios (CMSPAR+PARODD+PARENB) since the `serialport` crate lacks Mark parity support.
- `payload.rs` — Bidirectional payload codec: decode (bus→MQTT) and encode (MQTT→bus). Handles types: byte, word, int, char, date, time, enum. Multi-register payloads split using config-defined lengths.
- `config.rs` — Loads `device.yaml` (unit identity) + `registers.yaml` (register map) from the same directory, merges them, and builds register lookup maps and HA discovery metadata.
- `ha_discovery.rs` — Generates Home Assistant MQTT discovery payloads per entity.
- `mqtt.rs` — rumqttc async client wrapper with publish/subscribe and reconnection.

**Subcommands** (clap): `run` (default), `health-check`, `validate-config`.

## Bus Protocol Essentials

- Message format: `[addr_hi, addr_lo, cmd, length, crc_lo, crc_hi, ...payload]`
- Multi-register payloads: `[reg_addr, 0x00_separator, value_bytes..., reg_addr, 0x00, value_bytes...]`
- Write pings (payload ≤ 3 bytes in 0x85 commands) are status broadcasts, not actionable data — skip consuming them.
- After a write (0x85), bus devices respond with 0x86 (update_req) and 0x87 (checksum ACK echoing the write's CRC).

## Configuration

Register definitions live in `config/registers.yaml` (static register map). Per-unit identity (name, hw/sw version, variant) lives in `config/device.yaml`. Both are loaded together from the same directory by `config.rs::DeviceConfig::load`. Each entity has: address, payload config (length, type, options, decimals, signed), MQTT topic, operations (RO/RW), and optional HA discovery block. The bus protocol itself (command codes, quirks, 0x88 time-sync correlation) is documented in `BACKGROUND.md`.

Environment variables (loaded from `.env` via dotenvy). Grouped by area, defaults in `src/main.rs`:

- **Logging**: `LOG_LEVEL`, `LOG_FORMAT` (`human`|`json`), `LOG_FILE`, `ENABLE_FILE_LOGGING`.
- **MQTT**: `MQTT_BROKER`, `MQTT_PORT`, `MQTT_USERNAME`, `MQTT_PASSWORD`, `MQTT_BASE_PREFIX`, `MQTT_TLS`, `MQTT_TLS_INSECURE`, `MQTT_CLEAR_ON_START`.
- **Serial**: `SERIAL_PORT`, `SERIAL_BAUDRATE`.
- **Config files**: `NOVUS_CONFIG` (device.yaml path), `NOVUS_REGISTERS` (override registers.yaml path).
- **Home Assistant**: `HASS_DISCOVERY`, `HASS_DISCOVERY_PREFIX`, `HASS_DEVICE_DISCOVERY` (single bundled payload vs per-entity), `HASS_DISCOVERY_MODE` (`startup`|`lazy`).
- **Behavior**: `AUTO_SYNC_TIME`, `REQ_MEM_ON_STARTUP` (request 0x83 memory-dump at boot), `HIGHLIGHT_UNKNOWN_REGS`, `ACTIVITY_LOG`.
- **Throttling**: `THROTTLE_MAX_INTERVAL`, `THROTTLE_MIN_CHANGE_PCT`, `DISABLE_THROTTLE`.
- **Diagnostics**: `SUMMARY_INTERVAL`, `BUS_DUMP_FILE`, `MAX_MEMORY_MB`.

## Versioning

CalVer format `YYYY.M.D`, generated at build time by `build.rs` from git state. Accessed via `env!("BUILD_VERSION")` and `env!("BUILD_GIT_HASH")`.

## Target Platform

Runs on Raspberry Pi (aarch64 Linux). Single-instance enforced via file lock at `/tmp/novus2mqtt.lock`.

## Safety Rules (live hardware)

This project talks to the actual ventilation unit in the user's home over a single MQTT broker (`<broker>`). There is no dev/staging instance.

- **Never publish to `/set` topics without explicit user confirmation.** A `PreToolUse` hook ([.claude/hooks/mqtt-set-safety.sh](.claude/hooks/mqtt-set-safety.sh)) blocks `mosquitto_pub` to any `/set` topic. After the user approves a specific write, re-issue the command with a trailing `# CONFIRMED:<reason>` marker.
- `mosquitto_sub` (read-only) is never blocked — use it freely for inspection.
- Restarting `novus2mqtt` mid-cycle can interrupt a write the bus is mid-acknowledge. Prefer waiting for a `0x87` ACK before restart unless the user says otherwise.
- `device.yaml` and `registers.yaml` are config-as-code for live behaviour. Treat edits like code changes: `cargo run -- validate-config` after changes.

## Project Tooling

Loadable on-demand via the Claude Code skill/agent system (see [.claude/](.claude/)):

**Skills** (expertise that joins the conversation when triggered):
- `paul-novus-expert` — HVAC domain knowledge (registers, modes, errors, commissioning).
- `rust-senior` — Rust security-by-design and SSDLC mindset for code work.
- `bus-reverse-engineer` — methodology for analyzing serial bus dumps.
- `mqtt-ha-discovery` — conventions for adding/changing HA entities in `registers.yaml`.

**Subagents** (delegated, isolated context):
- `log-analyzer` (Haiku, read-only) — triages bus dumps and MQTT traces, returns a summary instead of raw bytes.

**Slash commands**:
- `/check` — full local CI gate (fmt → clippy → build → test).
- `/bus-dump <seconds>` — capture and summarize live bus traffic.
- `/release [note]` — cut a CalVer release (verify, CHANGELOG, tag, push).
