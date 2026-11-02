# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project uses Calendar Versioning (`YYYY.M.D`).

## [Unreleased] - 2026-04-16

### Added
- **MQTT publish throttling** per register: `throttle:` block with `min_change`,
  `min_change_pct`, `max_interval` (heartbeat), and `min_interval` (rate-limit ceiling).
  Global defaults via `THROTTLE_MAX_INTERVAL` / `THROTTLE_MIN_CHANGE_PCT`.
  Outcome tags `[published: new|changed|stale]`, `[skipped: delta|held|rate]`,
  `[omitted]` on every activity-log line.
  See [README § MQTT publish throttling](README.md#mqtt-publish-throttling).
- **Activity log** (`ACTIVITY_LOG=true`): per-register live console output as
  `<addr> <name> <- value  [outcome]` for RECV and `-> value` for SEND. Gated by
  both the env flag and `suppress_activity: true` in registers.yaml.
- **Unknown / TBC register highlighter** (`HIGHLIGHT_UNKNOWN_REGS=true`): bold
  magenta alert for unmapped registers and bold cyan for registers flagged
  `tbc: true`, once per session, to support reverse-engineering sessions.
- **Markers and bookmarks/flags**: press `m` for planned markers (`M1, M2…`) and
  `f` for unplanned bookmarks (`B1, B2…`). Counters reset at startup. Emitted
  both to stdout (colored — yellow/red) and the bus dump file.
- **Activity-stream control**: `p` pauses (buffered, replays on `r`), `b`
  blacks out (dropped), `r` resumes. All three continue MQTT/bus processing
  in the background; they only affect console display.
- **Highlight register**: `h` opens an alt-screen prompt to type a register
  address; matching activity lines render in bold bright white. `H` clears.
- **Keyboard `c` / `C`** clears the console screen. Uppercase variants of other
  keys (`D`, `T`, `S`, `M`, `F`, `P`, `B`, `R`) clear-then-act.
- **Per-log elapsed fields**: `[t+HH:MM:SS Δ--:--:--]` suffix on every tracing
  line — uptime + time since last marker (resets on `m`/`f` press).
- **Reverse-engineering scripts** in [`scripts/rev_eng/`](scripts/rev_eng/) that
  instruct the user through menu walks with marker cues.
- Consolidated bus protocol reference into [BACKGROUND.md](BACKGROUND.md)
  including the hourly 0x88 time-sync quirk and the 0x80/0x81 bus re-enumeration
  observed on TFT standby toggle.

### Changed
- **Config split**: `device.yaml` now contains only the per-unit identity
  (~20 lines); register definitions moved to a new `config/registers.yaml`.
  Auto-discovered next to `device.yaml`, overridable via `NOVUS_REGISTERS`.
- **Env var renamed**: `publish` yaml field → `mqtt_publish` (old name accepted
  via serde alias for back-compat).
- **Startup 0x83 dump is now opt-in** via `REQ_MEM_ON_STARTUP=true` (default off).
  Previously it ran unconditionally and briefly disturbed the TFT.
- Removed the dead `bus:` block from `device.yaml` — it was doc-only, the
  consume/skip logic is hardcoded in `app.rs`. Command-code reference moved to
  BACKGROUND.md.

### Removed
- **Hourly periodic 0x83 memory-dump** — redundant since the VMC rebroadcasts
  every register within ~30 s, and the dump briefly disturbed the TFT.

### Fixed
- **`word` payload type never decoded** (`config.rs` type dispatcher only
  handled `byte`, `int`, `char`, `date`, `time`, `counter`, `unknown`). Register
  `0x5b Fan Speed Nominal (mirror)` now publishes its value instead of empty.
- `0x63 Automatic Mode` activity line showed empty value — wire value `e0 01`
  wasn't in the enum options; added as "Automatic Timing" while keeping `e000`
  as an alias.
- `0x7f Defroster Type` missing `01: Electro Defroster` option.
- Decoding typos and precision fixes derived from a full startup snapshot: six
  registers (`0x45`, `0x48`, `0x5c`, `0x5d`, `0x61`, `0x7c`) gained `decimals: 1`
  so tenths-of-°C values render correctly. `0x7d`/`0x7e` now decode their
  1-byte "pointer-to-sensor-register" value as the `T1..T4` sensor name.
- Register name typos: `Non-Cccupation` → `Non-Occupation`; `Parametern_min`/
  `_max` → `Parameter n_min/n_max`.
- `0x62` renamed from the incorrect "Bypass Check" to "Active Fan-Speed Menu
  Index" (observed values 11/12/13 track FS1/FS2/FS3 edits, not bypass).

## [2025.4.14] - 2026-04-14

### Changed
- **Complete rewrite from Python to Rust** for improved performance and reliability
- Async runtime via `tokio` for non-blocking serial and MQTT I/O
- Serial communication via `tokio-serial` with native Mark parity support (termios CMSPAR)
- MQTT client via `rumqttc`
- Structured logging via `tracing`
- Configuration loading from `config/device.yaml` via `serde_yaml`
- CLI argument parsing via `clap` with `validate-config` and `health-check` subcommands
- Environment variables auto-loaded from `.env` via `dotenvy`
- CalVer versioning (`YYYY.M.D`) with `build.rs` generating `BUILD_VERSION`

### Added
- 149 fully mapped registers with Home Assistant MQTT auto-discovery
- Dynamic temperature sensor naming based on construction variant (left/right)
- Payload classes: `version`, `bool`, `code`, `bitwise`, `enum` with typed JSON output
- Enum entities include available options in MQTT payload
- Multi-register splitting for packed bus messages
- Bus dump recording to file (`BUS_DUMP_FILE` env var)
- Time sync support (`AUTO_SYNC_TIME` env var) — syncs system clock to Novus every hour
- Interactive keyboard commands (`d`=dump, `t`=time, `s`=stats, `q`=quit)
- `validate-config` subcommand for pre-flight configuration checks
- `health-check` subcommand for MQTT and serial connectivity verification
- Proper HA entity categorization (control / config / diagnostic)
- Write queue that waits for bus gaps (0x84/0x87) before sending
- Periodic memory dump refresh (hourly)
- Single-instance enforcement via file lock
- Release binary optimized with LTO, stripped symbols, `panic=abort`

### Removed
- Python codebase and all Python dependencies
- Docker support (Dockerfile, docker-compose.yml, healthcheck.sh)
- JSON config files (replaced by device.yaml)

## [2022.2.1] - Original Python Release

### Features
- RS485 serial communication with Paul Novus 300 ventilation units
- MQTT publishing of register values
- Basic Home Assistant integration
- Docker containerization
- Configuration via JSON files
