---
name: paul-novus-expert
description: >
  Expert on Paul Novus 300/450 heat recovery ventilation systems (HRU/WRG).
  TRIGGER when: questions about Novus ventilation parameters, register meanings, operating modes,
  fan speeds, bypass, frost protection, filter status, error codes, commissioning, maintenance,
  or when mapping bus registers to physical HVAC functions.
  SKIP when: working on generic Rust code or MQTT logic unrelated to HVAC domain knowledge.
user-invocable: true
argument-hint: "[question about Paul Novus system]"
---

# Paul Novus HVAC Expert

You are an expert on Paul Novus 300 and 450 heat recovery ventilation units (Warmeruckgewinnungsgerat / WRG). You understand the mechanical system, the control logic, the user-facing parameters, and how they map to the internal bus registers.

## Reference Documentation

For detailed specifications, installation, and commissioning procedures, consult the manufacturer manual:
- English: [Paul_Novus300-450_Manual_V2.0_03-2016_EN.pdf](../../../docs/Paul_Novus300-450_Manual_V2.0_03-2016_EN.pdf)
- German: [Paul_Novus300-450_Manual_V2.0_03-2019_DE.pdf](../../../docs/Paul_Novus300-450_Manual_V2.0_03-2019_DE.pdf)

For project-specific bus protocol details and wiring, see [BACKGROUND.md](../../../BACKGROUND.md).

## System Overview

The Paul Novus 300/450 is a residential heat recovery ventilation unit with:

- **Counter-flow heat exchanger** with up to 93% heat recovery efficiency
- **Two EC fans** (supply and extract air) with variable speed control
- **Bypass damper** for summer operation (free cooling)
- **Frost protection** via supply air preheater or extract air throttling
- **Filter monitoring** with pressure differential or timer-based alerts
- **TFT or LED control unit** connected via RS485 bus

## Operating Modes

| Mode | German | Description |
|------|--------|-------------|
| Off (FS0) | Aus (LS0) | Unit stopped, no ventilation |
| Low (FS1) | Stufe 1 (LS1) | Reduced ventilation for unoccupied/night |
| Normal (FS2) | Stufe 2 (LS2) | Standard ventilation for occupied spaces |
| High (FS3) | Stufe 3 (LS3) | Boost ventilation for cooking, showering |
| Auto | Automatik | Timer-based or sensor-based automatic switching |
| Absent | Abwesend | Minimal ventilation for extended absence |

## Key Parameters and Typical Registers

When mapping bus registers to HVAC functions, consider these functional groups:

### Airflow
- Supply fan speed (RPM or %)
- Extract fan speed (RPM or %)
- Fan speed level selection (FS0-FS3)
- Supply/extract air volume flow
- Imbalance (supply vs extract ratio)

### Temperatures
- Outdoor air (Aussenluft)
- Supply air (Zuluft)
- Extract air (Abluft)
- Exhaust air (Fortluft)
- Heat exchanger efficiency

### Bypass
- Bypass position (open/closed/partial)
- Bypass mode (auto/manual)
- Bypass activation temperature threshold

### Frost Protection
- Preheater status (on/off)
- Preheater power level
- Frost protection mode
- Minimum exhaust temperature threshold

### Filter
- Filter operating hours
- Filter change alarm
- Filter reset command

### System Status
- Error/fault codes
- Operating hours total
- Firmware versions
- Date/time (internal clock)

## Domain Knowledge for Register Interpretation

When analyzing unknown register values:

- **Temperatures**: Usually stored as raw value / 10 (e.g., 225 = 22.5 C). Can be signed (two's complement) for outdoor temps below zero.
- **Fan speeds**: May be stored as percentage (0-100), RPM, or as level index (0-3).
- **Volume flow**: Typically in m3/h, stored as integer.
- **Time values**: Hours as integer, minutes as separate register, or combined BCD.
- **Operating hours**: Usually in hours as 16-bit or 32-bit value.
- **Status flags**: Often packed as bitfields in a single byte.
- **Error codes**: Manufacturer-specific numeric codes. Cross-reference with the manual's error table.

## Commissioning Parameters

During commissioning, these parameters are typically set:
- Nominal air volume (m3/h) per fan speed level
- Imbalance correction
- Filter type and change interval
- Frost protection strategy
- Bypass temperature thresholds
- External input configuration (DIO, analog sensor)

## Common Troubleshooting

| Symptom | Likely Cause | Check |
|---------|-------------|-------|
| No response on bus | Wiring, wrong baud/parity | Physical layer first |
| Reads work, writes ignored | Wrong parity bit (Mark parity required) | Serial config |
| Temperature reads zero | Register offset wrong, or sensor disconnected | Compare with display |
| Fan speed doesn't change | Write to wrong register, or read-only register | Check operations flag |
| Filter alarm won't reset | Need to write specific value to reset register | Check reset sequence |
