# Architecture and System Design

## Overview

novus2mqtt is a Rust-based MQTT bridge that connects Paul Novus 300 ventilation systems to Home Assistant and other MQTT-based automation platforms. It communicates with the device via RS485 bus protocol and publishes/subscribes to MQTT topics.

## System Architecture

```
+---------------------+
|   Paul Novus 300    |
|  Ventilation Unit   |
+----------+----------+
           | RS485 Bus (9600 baud)
           |
+----------v----------+         +------------------+
|  Serial Interface   |         | Configuration    |
|   (tokio-serial)    |         | (device.yaml)    |
+----------+----------+         +------------------+
           |
+----------v--------------------------+
|   novus2mqtt (Rust binary)          |
|  - Bus protocol parser (protocol)  |
|  - Register mapping (config)       |
|  - MQTT client (rumqttc)           |
|  - HA discovery (ha_discovery)     |
|  - Metrics & diagnostics           |
+----------+--------------------------+
           | MQTT (1883)
           |
+----------v----------+         +------------------+
|  MQTT Broker        |         | Home Assistant   |
|  (Mosquitto)        |-------->| Instance         |
+---------------------+         +------------------+
```

## Core Modules

### 1. Serial Communication (`serial.rs`)
- Async serial I/O via `tokio-serial`
- Configurable port and baud rate via env vars (`SERIAL_PORT`, `SERIAL_BAUDRATE`)
- Continuous read loop feeding raw bytes to the protocol parser

### 2. Bus Protocol (`protocol.rs`)
- Parses incoming RS485 bus frames
- Message structure: `[addr_hi, addr_lo, cmd, length, crc_lo, crc_hi, ...payload]` — note the CRC is at bytes 4-5 (middle), not at the end. Computed over `bytes[0..4] + bytes[6..]`.
- Custom CRC16-CCITT (init=0, NOT the standard 0xFFFF) validation
- Multi-register splitting for multi-byte values
- Outgoing command construction for writable registers

### 3. Configuration (`config.rs`)
- Loads `config/device.yaml` (unit identity) plus the sibling `config/registers.yaml` (register map) at startup — path of the registers file overridable via `NOVUS_REGISTERS`
- Defines sensor and control registers with metadata, plus per-register flags for activity logging (`suppress_activity`), MQTT publish control (`mqtt_publish`), throttling (`throttle:` block), and reverse-engineering status (`tbc`)
- Provides HA discovery configuration per register
- Validates config structure (also available via `validate-config` subcommand)

### 4. MQTT Client (`mqtt.rs`)
- Async MQTT via `rumqttc`
- Publishes sensor state updates
- Subscribes to command topics for writable registers
- Handles connection, reconnection, and retained messages
- Optional clearing of retained topics on startup (`MQTT_CLEAR_ON_START`)

### 5. Home Assistant Discovery (`ha_discovery.rs`)
- Generates MQTT discovery payloads for each register
- Entity types: sensor, number, select
- Publishes to `{HASS_DISCOVERY_PREFIX}/{entity_type}/{device_id}/.../config`
- Device grouping so all entities appear under one device in HA

### 6. Application Loop (`app.rs`)
- Orchestrates serial reading, protocol parsing, MQTT publishing
- Handles interactive keyboard commands: `d`=dump, `t`=time, `s`=stats, `m`=marker, `f`=bookmark/flag, `p`=pause, `b`=blackout, `r`=resume, `h`=highlight register (prompt), `H`=clear highlights, `c`=clear screen, `?`=help, `q`/`x`=quit. Uppercase variants (`D`/`T`/`S`/`M`/`F`/`P`/`B`/`R`) clear the screen first. Pause buffers and replays the activity stream on `r`; blackout drops silently. Activity log can be highlighted per-address; MQTT publish is throttled per-register with deadband + heartbeat + rate-limit, each line tagged with an outcome.
- Coordinates NTP time sync if enabled (off by default — see `AUTO_SYNC_TIME` note in [BACKGROUND.md](BACKGROUND.md))
- Bus dump recording to file, including marker/bookmark annotations
- Per-register MQTT publish throttling (deadband + heartbeat + rate-limit)
- Activity log with `RECV` (`<-`) / `SEND` (`->`) arrows and outcome tags: `[published: new|changed|stale]`, `[skipped: delta|held|rate]`, `[omitted]`

### 7. Payload Formatting (`payload.rs`)
- Converts raw bus bytes to MQTT payload JSON
- Applies type-specific conversions (integer, float, enum, date, etc.)

### 8. Metrics & Diagnostics (`metrics.rs`)
- Tracks message counters, error counts, uptime
- Publishes diagnostics to `{base}/metrics` periodically

### 9. Health Check (`health.rs`)
- `cargo run -- health-check` subcommand
- Verifies MQTT broker connectivity
- Verifies serial port accessibility
- Reports pass/fail status

### 10. Logging (`logging.rs`)
- Structured logging via `tracing` + `tracing-subscriber`
- Configurable log level via `LOG_LEVEL` env var
- Supports JSON output format

### 11. Signal Handling (`signal.rs`)
- Graceful shutdown on SIGTERM/SIGINT
- Clean MQTT disconnect and serial port release

## Data Flow

### Receiving Sensor Data
```
1. Bus frame arrives on serial port
   -> serial.rs reads raw bytes
      -> protocol.rs parses frame, validates CRC16
         -> config.rs looks up register metadata
            -> payload.rs formats MQTT payload
               -> mqtt.rs publishes to {base}/{device_id}/{topic}/state
                  -> Home Assistant receives update
```

### Sending Control Commands
```
1. User changes value in Home Assistant
   -> HA publishes to {base}/{device_id}/{topic}/set
      -> mqtt.rs receives message
         -> config.rs resolves register address
            -> protocol.rs builds bus frame with CRC16
               -> serial.rs writes to serial port
                  -> Device applies the command
```

### Home Assistant Discovery
```
1. Application starts with HASS_DISCOVERY=true
   -> config.rs loads device.yaml + registers.yaml, merges them
      -> ha_discovery.rs generates discovery JSON per register
         -> mqtt.rs publishes to {HASS_DISCOVERY_PREFIX}/{type}/{device_id}/.../config
            -> Home Assistant auto-discovers entities
```

## Configuration Format

Configuration is split across two YAML files in `config/`:

- **`device.yaml`** — unit identity (manufacturer, model, hw/sw version, serial,
  variant left/right). User-edited per unit.
- **`registers.yaml`** — the static register map (sensors, controls, payload
  types, MQTT topics, HA discovery). Not typically user-edited.

Both are loaded together by `DeviceConfig::load`; `registers.yaml` is
auto-discovered next to `device.yaml` (or set `NOVUS_REGISTERS` to override).

### device.yaml
```yaml
device:
  name: Paul Novus 300
  manufacturer: Paul Novus
  model: NOVUS300
  variant: left           # "left" or "right" — determines temperature sensor mapping
```

### registers.yaml
```yaml
sensors:
  - address: '0x00'
    payload:
      length: 3
      type: byte
      class: version
    topic: bus/system/bus_version
    name: BUS Version
    unique_id: sys_bus_version
    operations: RO
    home_assistant:
      type: sensor
      config:
        entity_category: diagnostic
        icon: mdi:information-outline

controls:
  - address: '0x28'
    payload:
      length: 1
      type: byte
      class: enum
      options:
        '06': Auto
        '07': Level 1
    topic: bus/ventilation/selection
    name: Ventilation Selected Level or Preset
    unique_id: vent_selection
    operations: RW
    command_topic: bus/ventilation/selection/set
    home_assistant:
      type: select
      config:
        options:
        - Auto
        - Level 1
        cmd_t: ~/bus/ventilation/selection/set
```

## Error Handling

- `anyhow` for application-level errors with context
- `thiserror` for typed domain errors
- CRC16 validation rejects corrupted bus frames
- MQTT reconnection with backoff on broker disconnect
- Invalid register values logged and skipped
- Config validation prevents startup with bad configuration

## Performance Characteristics

- **Memory**: Low footprint, single-binary with no runtime interpreter
- **CPU**: Minimal; async I/O via tokio avoids busy-waiting
- **Serial**: 9600 baud (~1.2 KB/s max)
- **Release binary**: Optimized with LTO, stripped symbols, size-optimized (`opt-level = "s"`)

## Security Considerations

1. **MQTT Authentication**: Username/password via env vars
2. **Environment Variables**: Credentials in `.env`, never committed to git
3. **Serial Port**: Restricted to configured device path
4. **Message Validation**: CRC16 ensures bus frame integrity
5. **No network listeners**: The binary only makes outbound connections (MQTT, serial)
