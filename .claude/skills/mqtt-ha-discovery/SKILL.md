---
name: mqtt-ha-discovery
description: >
  Conventions for adding or modifying Home Assistant MQTT discovery entities in this codebase
  (registers.yaml + ha_discovery.rs). TRIGGER when: adding a new register entry that should
  appear in HA, changing an entity's type/unit/options, debugging entities that show up wrong
  in HA (Unavailable, no unit, wrong device class, missing command topic). SKIP when: working
  on bus protocol, MQTT transport, or non-HA-facing logic.
user-invocable: true
argument-hint: "[entity name or topic to work on]"
---

# Home Assistant MQTT Discovery — novus2mqtt conventions

This codebase emits HA discovery using the **component-per-entity** model and a **device-bundle**
model in parallel (`ha_discovery.rs::generate_discovery` and `generate_device_discovery`). Read
that file when in doubt; the rules below are what's actually implemented.

## Where things live

- **Entity definition**: `config/registers.yaml` — one YAML doc, top-level lists `sensors:`,
  `actuators:`, etc. Each entry has `address`, `payload`, `topic`, `name`, `unique_id`,
  `operations` (RO/RW), and an optional `home_assistant:` block.
- **Discovery emission**: `src/ha_discovery.rs` — converts each entity into a discovery payload
  with auto-inferred metadata.
- **Per-unit identity**: `config/device.yaml` — name, hw/sw version, variant. NOT entity-level.

## Minimal entry checklist (sensor)

```yaml
- address: '0x42'
  payload:
    length: 2
    type: word          # byte | word | int | char | date | time | enum
    class: temperature  # informational; doesn't drive HA inference
    decimals: 1         # for float-like display
    signed: true        # for int
  topic: bus/temperatures/supply
  name: Supply Air Temperature
  unique_id: temp_supply
  operations: RO
  home_assistant:
    type: sensor        # sensor | binary_sensor | switch | select | number | button
    # `config:` block is optional — auto-inference fills in most fields. Only override.
```

## Auto-inference (what you DON'T need to set)

`apply_inferred_metadata` in [ha_discovery.rs:331](../../../src/ha_discovery.rs#L331) fills in:

- **Topic-based**: `temperature*` → `°C`, `dev_cla=temperature`, `stat_cla=measurement`,
  `sug_dsp_prc=1`, `mdi:thermometer`. Same for `humidity` (%, `mdi:water-percent`),
  `pressure` (`dev_cla=pressure`), `frost`/`defrost` (`mdi:snowflake`), `bypass` (`mdi:pipe`),
  `version` (`mdi:information-outline`, diagnostic), `error`/`message`
  (`mdi:alert-circle-outline`).
- **Payload-type-based** (sensor only): `float`/`int`/`uint`/`counter` → `stat_cla=measurement`;
  `counter` → `stat_cla=total_increasing`; `float` → `sug_dsp_prc=1`.
- **Enum sensors** (have `payload.options`): `dev_cla=enum`, `options` populated automatically.
- **Diagnostic tokens** in topic (`version`, `error`, `message`, `memory`, `diagnostic`):
  `entity_category=diagnostic` auto-applied.
- **Topics**: state topic auto-derives to `~/{topic}/state`; for RW entities, command topic
  auto-derives to `~/{topic}/set`. Don't hardcode unless you need a non-standard layout.

## When to override (`home_assistant.config:` block)

Only set fields the inference can't get right:

- `entity_category: diagnostic | config` — for entities not directly inferable
- `expire_after: <seconds>` — **opt-in only**. Most registers are pushed only on startup
  memory dump or when changed, so a global default would flap entities to *Unavailable*.
  Use `expire_after` ONLY for registers genuinely broadcast continuously.
- `enabled_by_default: false` — for noisy/internal sensors that pollute the HA dashboard
- `options:` — for `select` entities (the writable user-facing list)
- `min`/`max`/`step` — for `number` entities (step defaults to 1)
- `payload_on`/`payload_off` — for `switch` entities (state_on/state_off auto-derive from these)
- `icon` — only if topic-based inference picks the wrong one

## Per-entity flags outside `home_assistant`

- `expose_ha: false` — entity exists on the bus and gets MQTT-published, but no HA discovery.
  Use for unconfirmed/raw registers (`tbc: true`).
- `publish: false` — don't even publish to MQTT (rare).
- `suppress_activity: true` — don't log every receipt (for chatty registers).
- `throttle.max_interval: <seconds>` — debounce frequent updates.
- `tbc: true` — "to be confirmed", reverse-engineering in progress; usually paired with
  `expose_ha: false`.

## Entity type cheat sheet

| Type | Use for | Notes |
|---|---|---|
| `sensor` | Numeric / enum read-only | Auto-stat_cla for numeric payloads |
| `binary_sensor` | Boolean read-only | Set `dev_cla` for nice icons (`door`, `motion`, `problem`) |
| `switch` | Boolean RW | `payload_on`/`payload_off` drive both write and inferred state |
| `select` | Enum RW | Set `options:` in `home_assistant.config`; `cmd_tpl` defaults to `{{ value }}` |
| `number` | Numeric RW | Set `min`/`max`/`step`/`mode` (`box`/`slider`); `step` defaults to 1 |
| `button` | Stateless RW trigger | `entity_category` defaults to `config` |

## Common pitfalls

1. **RW entity has no command topic in HA** → check `operations: RW` is set; the command
   topic auto-derives only for RW.
2. **Number entity refuses values** → confirm `min`/`max` cover the range and `step` divides
   the allowed value range cleanly.
3. **Select entity grays out** → the `options` list in `home_assistant.config.options` must
   match exactly the strings published to the state topic.
4. **Sensor stuck on "Unknown"** → likely the value template doesn't match the payload.
   Default template is `{{ value_json.converted if value_json.converted is defined else value_json.hex }}`.
   Check what `payload.rs` actually emits for this `payload.type`.
5. **Entity flapping Unavailable** → you set `expire_after`; remove it unless the register is
   genuinely broadcast on a known cadence.
6. **Diagnostic entities cluttering main dashboard** → set `entity_category: diagnostic`.

## Verification workflow

1. Edit `config/registers.yaml`.
2. `cargo run -- validate-config` — catches structural errors.
3. `cargo build && ./target/debug/novus2mqtt` (or restart your running instance).
4. Watch the discovery topic land:
   `mosquitto_sub -h <broker> -u <user> -P <password> -t 'homeassistant/+/+/+/config' -v -C 1`
5. In Home Assistant: Settings → Devices & Services → MQTT → Devices → find the unit, verify
   the new entity appears with correct unit/icon/category.

## Reference

- HA discovery spec: https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery
- HA device classes: https://www.home-assistant.io/integrations/sensor/#device-class
- This codebase's emitter: [src/ha_discovery.rs](../../../src/ha_discovery.rs)
- Live examples: skim [config/registers.yaml](../../../config/registers.yaml) for entries
  matching the entity type you're adding.
