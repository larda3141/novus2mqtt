# Background Information

Project repository: https://github.com/larda3141/novus2mqtt

## Terminology

| English | German |
|---|---|
| HRU = Heat Recovery Unit | WRG = Warmeruckgewinnungsgerat |
| Control Modules = Control Units (LED or TFT) + External Inputs | Steuerungsmodule = Bedieneinheit (LED oder TFT) + Externe Eingange |
| Supply air and extract air mode | Zu- und Abluft Modus |
| Fan Speed (FS0-FS3) | Lufterstufen (LS0-LS3) |
| Un-occupied mode | Abwesend-Modus |
| Automatic mode (timing mode or sensor mode) | Automatik-Modus (Zeitautomatik oder Sensorautomatik) |
| Messages | Meldungen |

## Basic Information

### Paul Novus Interfaces

#### Through Terminal X10

| Terminal | Pin | Function | Contact Description | Notes |
|---|---|---|---|---|
| X10 | 1 | Status relay | Normally open contact (max. 24V switching voltage) | Fan on = closed |
| X10 | 2 | Status relay | Changeover contact (max. 24V switching voltage) | |
| X10 | 3 | Boost ventilation switch | Wire 1 | |
| X10 | 4 | Boost ventilation switch | Wire 2 (GND) | |
| X10 | 5 | External release (bridge) | External release | Factory-bridged. If removed: TFT blocked, LED L1+L7 blink |
| X10 | 6 | External release (bridge) | External release (GND) | |
| X10 | 7 | Digital I/O | DIO1 (activate automatic mode) | Factory: digital input |
| X10 | 8 | Digital I/O | DIO1 (GND) | DIO2: message output (digital output) |
| X10 | 9 | External sensor (analog input) | Wire 1 (sensor signal 0-10V or 4-20 mA) | |
| X10 | 10 | External sensor (analog input) | Wire 2 (GND) | |

#### RS485 BUS Connector X1

| Pin | Wire Color | Function |
|---|---|---|
| 1 | Red | 24V Power | 
| 2 | White | RX (Data) |
| 3 | Yellow | TX (Data) |
| 4 | Black | GND |
| 5 | Silver | Shield |

Cable type: J-Y(ST)Y 2x2x0.6

For the test environment hardware details (Raspberry Pi, USB adapter, wiring), see [SETUP.md](SETUP.md).

## Reverse Engineering: Bus Protocol

### Community Resources

The following resources from other reverse engineering efforts were instrumental in building both this application and its documentation:

- [Schack17's PaulFocusNovus Wiki](https://github.com/Schack17/PaulFocusNovus/wiki/RS485-Bus-Paul-Novus-300-(Focus-200)) — Register map, CRC algorithm, packet structure
- [Haustechnikdialog Forum](https://www.haustechnikdialog.de/Forum/t/153632/Paul-Novus-und-der-RS485-Anschluss) — Physical layer details, parity discovery, fan speed command sequences
- [KNX User Forum (ComfoAir/Paul)](https://knx-user-forum.de/forum/supportforen/smarthome-py/31291-neues-plugin-comfoair-kwl-wohnrauml%C3%BCftung-zehnder-paul-wernig) — 9-bit MPCM mode explanation

### Physical Layer

The Paul Novus uses an **RS485 bus** at the X1 connector. Connection to a host system (Raspberry Pi, PC) requires a USB-to-RS485 adapter.

**Serial port settings:**

| Parameter | Value | Notes |
|---|---|---|
| Baud rate | 9600 | |
| Data bits | 8 | See 9-bit mode note below |
| Parity | Mark | See parity note below |
| Stop bits | 1 | |
| Flow control | None | |

**9-bit mode (MPCM):** The Paul Novus master uses Atmel ATMEGA32 microcontrollers with 9-bit Multi-Processor Communication Mode (MPCM). The 9th bit distinguishes address frames (bit=1) from data frames (bit=0). With standard 8-bit UART adapters, this 9th bit appears as the parity bit.

**Parity behavior:** The parity bit is set to Mark (1) for the first two bytes (the address pair) and Space (0) for all subsequent bytes. This corresponds to the MPCM protocol where address bytes have the 9th bit set. Standard USB-RS485 adapters with 8N1 or 8-Mark-1 configuration can read the bus correctly. Writing requires sending the first two bytes with Mark parity, which most USB adapters don't support natively.

**Practical note:** The Python `serial` library uses `parity=serial.PARITY_MARK` which sets Mark for ALL bytes. The Rust port also sets Mark parity for all bytes, but via raw Linux termios (`CMSPAR + PARODD + PARENB`) since the `serialport` crate only exposes None/Odd/Even — see [src/serial.rs:200-226](src/serial.rs#L200-L226). Both work for **reading** the bus. **Writing** may fail because the master expects the 9th bit set only on address bytes. This is a known limitation that may explain why register writes (especially date/time) are not accepted by the VMC — see "Known Limitations" below.

### Message Format

All bus messages follow this structure:

```
Byte:  [0]    [1]     [2]    [3]      [4]   [5]    [6..N]
Field: ADDR0  ADDR1   CMD    LENGTH   CRC_L CRC_H  DATA...
```

| Byte | Name | Description |
|---|---|---|
| 0 | Address byte 0 | `0x00` or `0x01` (bus direction indicator) |
| 1 | Address byte 1 | Device address (`0x00`-`0x0C`, `0xFF`) |
| 2 | Command | Command code (see table below) |
| 3 | Length | Payload length. If `>= 0x80`, actual length = `value - 0x80`. Exception: command `0x87` always has 3 bytes of payload regardless of length byte |
| 4 | CRC low | Checksum LSB |
| 5 | CRC high | Checksum MSB |
| 6..N | Data | Payload data (length determined by byte 3) |

Total message size = 6 (header) + payload length.

**Important:** The checksum bytes are in the **middle** of the message (bytes 4-5), not at the end. This is unusual compared to most protocols.

### Byte Order

All multi-byte numeric values on the bus use **little-endian** byte order:

```
Example: Temperature 22.0C = 220 decimal = 0x00DC
On bus: [0xDC, 0x00] (LSB first)
```

### Checksum (CRC)

The bus uses a **custom CRC16-CCITT** algorithm with initial value `0x0000` (not the standard `0xFFFF`):

```
CRC16-CCITT (init=0):
  msb = crc >> 8
  lsb = crc & 0xFF
  for each byte c in data:
    x = c XOR msb
    x = x XOR (x >> 4)
    msb = (lsb XOR (x >> 3) XOR (x << 4)) AND 0xFF
    lsb = (x XOR (x << 5)) AND 0xFF
  return (msb << 8) + lsb
```

The checksum is computed over `header[0:4] + data`, excluding the checksum bytes themselves. The result is stored LSB-first at bytes 4-5:

```
checksum_input = [ADDR0, ADDR1, CMD, LENGTH] + [DATA...]
crc = crc16_ccitt(0, checksum_input)
byte[4] = crc & 0xFF       (LSB)
byte[5] = (crc >> 8) & 0xFF (MSB)
```

### Valid Device Addresses

| Address | Device |
|---|---|
| `0x00` | Master (VMC controller) |
| `0x01` | Main device / Self |
| `0x02` | Slave (fan controller) |
| `0x03`-`0x0C` | Other peripherals (TFT panels, LED panel, defroster, heater, etc.) |
| `0xFF` | Broadcast / new device discovery |

### Command Codes

| Code | Name | Direction | Description | Skip Consume |
|---|---|---|---|---|
| `0x00` | Clock | Master -> Bus | Clock synchronization | No |
| `0x80` | Join Request | Master -> Device | "Do you want to join?" (includes bus version) | Yes |
| `0x81` | Join Response | Device -> Master | "I am in!" (includes version info) / Master -> Device: "Welcome!" (acknowledgement, no data) | Yes |
| `0x83` | Memory Dump | Master -> Memory | Request full memory dump. Triggers burst of `0x85` messages | No |
| `0x84` | Ping | Device -> Master | Keepalive / "Give me a ping!" | Yes |
| `0x85` | Memory Write | Master -> Memory | **Main data command.** Contains register data (single or packed multi-register) | No |
| `0x86` | Update Request | Device -> Master | "Any updates for me?" | Yes |
| `0x87` | Checksum Ack | Master -> Device | Acknowledge with checksum from previous `0x85`. Always 3 bytes payload | Yes |
| `0x88` | Checksum Error | Device -> Master | Checksum mismatch notification | Yes |

"Skip Consume" means the message is a protocol control message and does not contain register data to publish.

### Bus Timing

- **Write-after-command:** Bus writes (register changes, dump requests) are only sent after receiving a `0x84` (Ping) or `0x87` (Checksum Ack) command. This ensures the write happens during a gap in the protocol, not mid-conversation.
- **Ping interval:** Master sends keepalive pings (`0x84`) to each registered slave approximately every 100ms.
- **Broadcast ping:** Master pings address `0xFF` approximately every 9 seconds (looking for new devices).
- **Registration timeout:** ~160ms after last registration request on the last slave address.

### Memory Write (0x85) Payload Format

The `0x85` command carries register data. A single message can contain **multiple registers packed together**.

#### Single Register

```
[register_address, 0x00, value_byte_0, value_byte_1, ...]
```

Example: Register `0x28` (ventilation level) with value `0x06` (Auto):
```
[0x28, 0x00, 0x06]
```

#### Multi-Register (Packed)

Multiple registers are concatenated in a single payload:

```
[addr1, 0x00, val1..., addr2, 0x00, val2..., addr3, 0x00, val3...]
```

Example (19 bytes, 5 registers packed):
```
44 00 5c 81 00 dc 00 82 00 64 00 83 00 e1 00 84 00 73 00
```
Decoded using config-defined payload lengths:
| Register | Separator | Value (hex) | Value (decoded) |
|---|---|---|---|
| `0x44` | `0x00` | `5c` | 92 (unknown register) |
| `0x81` | `0x00` | `dc 00` | 220 -> 22.0 C (Temperature T2) |
| `0x82` | `0x00` | `64 00` | 100 -> 10.0 C (Temperature T1) |
| `0x83` | `0x00` | `e1 00` | 225 -> 22.5 C (Temperature T4) |
| `0x84` | `0x00` | `73 00` | 115 -> 11.5 C (Temperature T3) |

**Splitting logic:** The receiver must use the **configured payload length** for each register address to determine where one register ends and the next begins. The `0x00` separator byte is ambiguous (it can also be valid data), so length-based splitting is the only reliable method.

#### Write Pings (Length <= 3)

`0x85` messages with payload length <= 3 are "write pings" -- status checks, not data. These are sent to specific addresses (e.g., `0x04`, `0x08`, `0x0E`, `0x1A`, `0x1D`, `0x44`) and should be **skipped** during data consumption.

### Register Data Types

Each register has a configured payload type that determines how to decode the raw bytes:

| Type | Length | Encoding | Example |
|---|---|---|---|
| `byte` | 1 | Unsigned integer, little-endian | `0x42` -> 66 |
| `word` | 2 | Unsigned integer, little-endian (2-byte alias of `byte`/`int`; always unsigned) | `[0x32, 0x00]` -> 50 |
| `int` | 1-2 | Integer, little-endian, optionally signed | `0xF6` (signed) -> -10 |
| `char` | N | ASCII string, null-terminated | `0x53 0x57 0x5A` -> "SWZ" |
| `date` | 4 | `[0x00, day, month, year-2000]` | `[0x00, 0x0E, 0x04, 0x1A]` -> 2026-04-14 |
| `time` | 3 | `[second, minute, hour]` | `[0x2D, 0x1E, 0x15]` -> 21:30:45 |
| `counter` | 5 | Duration: `[minutes, hours, days_lo, days_hi(?), years]` | Converted to total seconds |
| `enum` | 1-2 | Hex value mapped to string via options table | `0x06` -> "Auto" |

**Decimals:** For `int`/`word`/`byte` types with `decimals: N`, the raw value is divided by `10^N`. Example: raw value `220` with `decimals: 1` -> `22.0`.

**Signed integers:** When `signed: true` (only meaningful for `int`), values are interpreted as two's complement. Example: 1-byte `0xF6` -> `-10`. `word` and `byte` are always unsigned.

### Outgoing Message Construction

To write a register value to the bus:

```
1. Build header: [dest_addr_0, dest_addr_1, command, length]
2. Build data:   [register_address, 0x00, encoded_value_bytes...]
3. Compute CRC:  crc16_ccitt(0, header + data)
4. Assemble:     header + [crc_lo, crc_hi] + data
5. Queue and send after next 0x84/0x87 gap
```

For a memory dump request (command `0x83`):
```
header = [0x01, 0x01, 0x83, 0x00]
crc = crc16_ccitt(0, header)
message = header + [crc_lo, crc_hi]
```

### Boot Sequence

1. Master sends registration request (`0x80`) to each device address (`0x01` to `0x0C`), including the bus version
2. If no slave responds within ~160ms, the request is repeated
3. If a slave responds (`0x81`), it includes its bus version, device type, and HW/SW version
4. Master acknowledges the registration with `0x81` (no data), ~50ms after the response
5. Registration closes ~160ms after the last request by sending `0x83` (memory dump request)
6. Master begins sending keepalive pings (`0x84`) to each registered slave every ~100ms
7. Master pings broadcast address `0xFF` every ~9s for new devices

**On VMC crash/reset:** No full registration sequence -- just a registration close (`0x01 0x00 0x83`) and acknowledge (`0x87`) with checksum `0x00 0x00`.

### Protocol Flow: Normal Operation

```
Master -> Slave:   0x84 (Ping - "Are you alive?")
Slave  -> Master:  0x86 (Update Request - "Any updates for me?")
Master -> Slave:   0x87 (Checksum Ack - confirms current state)

Master -> Memory:  0x85 (Memory Write - register data, single or packed)
  Each slave acknowledges with 0x86
  Master confirms with 0x87 (checksum from 0x85 as data)

[If write queued]: After receiving 0x84 or 0x87, send queued write
```

### Protocol Flow: Writing a Register

```
1. User sends command via MQTT (e.g., set ventilation level to Auto)
2. Application encodes value to bus format (0x06 for Auto)
3. Application builds bus message: [0x01, 0x00, 0x85, len, crc_lo, crc_hi, 0x28, 0x00, 0x06]
4. Message is queued in write stack
5. On next 0x84 (Ping) or 0x87 (Checksum), the queued message is sent to the serial bus
6. Master processes the write and distributes to slaves
```

### Notes

- Sending with `0x85` does NOT require manual `0x86`/`0x87` acknowledgement from the bridge
- If VMC crashes, addresses `0x3D, 0x5A, 0x3E, 0x3F, 0x40-0x4B` are reset to defaults (sent in a single `0x85`)
- Command `0x88` appears to be an error response when writing with `0x85` and acknowledging with `0x87` using a non-existent checksum

### Observed: 0x88 fires only on our hourly time-sync (2026-04-16 dump)

In an 8-hour runtime capture, `0x88` was emitted **exclusively** in response to our hourly
AUTO_SYNC_TIME writes at HH:49. The surrounding bus traffic (~98 Hz of `0x84`/`0x85`) was
otherwise uninterrupted — there was **no multi-second silence** indicating an MCU reboot.
Two distinct `0x88` variants appeared:

| Frame | Source sub-module | Pattern |
|---|---|---|
| `01 08 88 00 24 4d` | Boot-phase (addr `0x08`) | **10-second 1 Hz burst** on the first two hourly syncs after every app startup; none thereafter. Likely the module taking ~10 s to accept the clock write while still "warming up". |
| `01 04 88 00 45 38` | Sub-version (addr `0x04`) | Single frame per hourly sync from ~4th hour onward. Likely a "nothing to do / checksum not recognised" response. |

**Implication for operators:** reverse-engineering captures taken across HH:49 will be
contaminated by ~20 s of this activity. Either run between HH:00-HH:40, or set
`AUTO_SYNC_TIME=false` for the capture. The proper fix is to make the time-write
conditional (compare wall-clock against the `0x00` clock broadcasts and skip when in sync).

### Known Limitations: Writing to the Bus

**Register writes may not be accepted by the VMC.** Multiple community members have reported that sending 0x85 write commands from external devices has no effect, even though the messages are correctly formatted with valid CRC.

Possible causes (based on community research):

1. **9-bit MPCM parity:** The master expects the 9th bit (parity/Mark) set on address bytes only. Standard USB-RS485 adapters either set Mark parity on ALL bytes (Python `PARITY_MARK`) or NONE. Neither matches the protocol exactly. The master may reject frames with incorrect parity on non-address bytes.

2. **Protocol handshake:** The master may require a specific handshake or device registration (0x80/0x81 sequence) before accepting writes from a bus participant. Our bridge is passive — it reads without registering as a device.

3. **Timing windows:** Writes may need to occur within a specific timing window relative to the master's polling cycle, not just "after any 0x84/0x87".

4. **TFT panel arbitration:** The TFT touch panel may hold exclusive write access. Some users report writes only work with the TFT disconnected (unconfirmed).

**Current status:** Reading all registers works perfectly. Writing to registers like ventilation level (0x28) was functional in the original Python implementation (reported by the author), but writing to date/time registers (0x01, 0x02) has never been confirmed working. The Rust port sends correctly formatted messages that appear in the bus dump but are not applied by the VMC. Further protocol analysis needed.

## HVAC Reference (from Paul Novus Manual V2.0)

### Temperature Sensors

The unit has 4 internal NTC temperature sensors. The mapping depends on the construction version:

| Sensor | Version LEFT | Version RIGHT |
|---|---|---|
| T1 | Intake air / Outdoor (Aussenluft) | Extract air (Abluft) |
| T2 | Supply air (Zuluft) | Exhaust air (Fortluft) |
| T3 | Extract air (Abluft) | Intake air / Outdoor (Aussenluft) |
| T4 | Exhaust air (Fortluft) | Supply air (Zuluft) |

Additional sensors (optional, not on standard bus registers):
- T5: `t_aut` — Outside temperature (external sensor)
- T6: `t_sde` — Brine defroster inlet temperature
- T7: `t_nhz` — Supply heater outlet temperature
- T8: `t_rth` — Room thermostat temperature

### Bypass Switching Logic

The bypass flap controls free cooling (summer) and heating bypass. Switching conditions (example for LEFT version):

**Bypass OPEN** (cooling): `T1 < T3 AND T1 > t_int_min AND T3 > t_ext_max + H_ext`
**Bypass OPEN** (heating): `T1 > T3 AND T3 < t_ext_min - H_ext`
**Bypass CLOSED** (heating): `T1 < T3 AND T3 < t_ext_max - H_ext`
**Bypass CLOSED** (re-cooling): `T1 > T3 AND T3 > t_ext_min + H_ext`

The bypass also stays closed below the intake air limit temperature (factory 13°C, configurable 12-20°C).

| Parameter | Description | Factory Default |
|---|---|---|
| t_ext_max | Upper threshold (max extract air temperature) | 25.0°C |
| t_ext_min | Lower threshold (min extract air temperature) | 18.0°C |
| H_ext | Hysteresis for extract air thresholds | 0.5 K |
| t_int_min | Intake air limit (bypass closed below this) | 13.0°C |

### Frost Protection

Three modes with different anti-freeze protection thresholds:

| Mode | Threshold | Description |
|---|---|---|
| Eco | -2.0°C | Residual risk of heat exchanger freezing; lower energy use |
| Safe | 0.0°C | Freezing absolutely prevented; higher energy use |
| Moisture Heat Exchanger | -8.0°C | For enthalpy exchangers (membrane type); freezing basically prevented |

Additional threshold: Minimum supply air temperature = 5.0°C (factory default).

When intake air falls below the anti-freeze threshold, the defroster is activated. If it remains below, fans are switched off and an error message is generated.

### Fan Speed Factory Defaults

#### TFT Touch Panel (FS1-FS3)

| Parameter | Factory Default | Range | Step |
|---|---|---|---|
| FS1 (reduced) | 20% | 17-100% | 1% |
| FS2 (nominal) | 50% | 17-100% | 1% |
| FS3 (purge/boost) | 80% | 17-100% | 1% |
| Constraint | | 17% < FS1 < FS2 < FS3 < 100% | |

#### LED Control Panel (7 levels)

| Level | NOVUS 300 (LEFT) | NOVUS 450 | NOVUS 300 (RIGHT) | NOVUS 450 (RIGHT) |
|---|---|---|---|---|
| 1 | 17% | 25% | 17% | 20% |
| 2 | 29% | 39% | 29% | 32% |
| 3 | 41% | 50% | 44% | 43% |
| 4 | 53% | 61% | 57% | 53% |
| 5 | 65% | 74% | 70% | 65% |
| 6 | 74% | 84% | 81% | 74% |
| 7 | 100% | 100% | 100% | 100% |

#### Imbalance (supply vs extract fan balance)

| Group | Fan Speeds | Range | Factory Default |
|---|---|---|---|
| Group 1 | FS1 + FS2 + reduced | -50% to +50% | 9% |
| Group 2 | FS3 + FS4 + FS5 + nominal | -50% to +50% | 15% |
| Group 3 | FS6 + FS7 + purge | -50% to +50% | 17% |

At centre position (0%), supply and extract fans run at the same speed.

### Air Volume Flow

| Model | Air Flow Range | Nominal (at FS2) |
|---|---|---|
| NOVUS (F) 300 | 45 - 300 m3/h | ~150 m3/h at 50% |
| NOVUS (F) 450 | 50 - 450 m3/h | ~225 m3/h at 50% |

### Automatic Timing Mode

The weekly time program uses 4 fan speed levels (FS0-FS3) with 15-minute resolution per day. The schedule is stored as 3 registers per day (8 hours each), 7 days = 21 registers.

Each 8-byte register encodes 32 quarter-hour slots using 2 bits per slot:
- `00` = Off (FS0)
- `01` = Level 1 (FS1 / reduced)
- `10` = Level 2 (FS2 / nominal)
- `11` = Level 3 (FS3 / purge)

**Factory defaults:**
- Mon-Fri: FS2 from 00:00-08:00, FS1 from 08:30-16:00, FS2 from 16:00-24:00, FS3 from 08:00-08:30
- Sat-Sun: FS2 all day (00:00-24:00)

Day groups (Mon-Fri, Sat-Sun) can be set together; individual day profiles override group settings.

### Automatic Sensor Mode

Uses an external air quality sensor (CO2, humidity, or temperature) connected to terminal X10.9/X10.10. The sensor's analog output drives a linear characteristic curve for fan speed:

| Sensor Type | Signal Range | Factory Defaults |
|---|---|---|
| Voltage (0-10V) | U_min=8.0V / n_min=17%, U_max=1.7V / n_max=80% | Inverted: higher voltage = lower fan speed |
| Current (4-20mA) | I_min=5.0mA / n_min=20%, I_max=19.0mA / n_max=80% | Higher current = higher fan speed |

Plausibility check (current sensors only): error if I < 3mA for >1s, reset if I > 3.5mA for >1s.

### Other Factory Defaults

| Parameter | Factory Default |
|---|---|
| Language | German |
| Boost duration | 15 min (range: 15-120 min, step: 5 min) |
| Unoccupied ventilation intensity | 15 min/h (range: 15/30/45 min/h) |
| Filter run-time | 90 days (range: 30-180 days, step: 10 days) |
| Filter warning threshold | 10 days remaining |
| Screen dimming degree | 75% (range: 5-95%, step: 5%) |
| Screen dimming duration | 1 min (range: 1-10 min) |
| Bypass mode | Active |
| Post heating type | None |
| Defroster type | PTC Defroster |
| GHE flap | Unavailable |
| Brine defroster cooling threshold | 22.0°C |
| Post heating duct temperature | 50.0°C |
| Key lock password | 11111 |

### Technical Specifications

| Specification | NOVUS (F) 300 | NOVUS (F) 450 |
|---|---|---|
| Heat recovery rate (PHI) | 93% (300) / 84% (F300) | 89% (450) |
| Heat recovery rate (DIN 4719) | — | 116% (F450) |
| Air flow range | 45-300 m3/h | 50-450 m3/h |
| Connected load | 0.14 / 1.44 kW (with defroster) | 0.36 / 1.66 kW (with defroster) |
| Sound emission (at 200 m3/h) | 21 dB(A) | 25 dB(A) |
| Weight | 50 / 52 kg (with defroster) | 50 / 52 kg (with defroster) |
| Electrical connection | 230 VAC, 50-60 Hz | 230 VAC, 50-60 Hz |
| Operating temperature | -20 to 40°C | -20 to 40°C |
| Pipe connections | DN 160 | DN 160 |
| Protection class / degree | I / IP 40 | I / IP 40 |

### Device Variants

| Version | Heat Exchanger | Frost Protection Modes |
|---|---|---|
| NOVUS 300/450 | Counter-flow channel (plastic) | Eco, Safe |
| NOVUS F 300/450 | Counter-flow enthalpy (polymer membrane) | Eco, Safe, Moisture Heat Exchanger |

Construction types: Vertical or Horizontal, Left or Right air duct layout.

## Error Codes

The error code register (16 bytes) stores: byte 0 = error code, bytes 1-15 = ASCII message text.

### TFT Touch Panel Error Messages

| Code | Display Message | Possible Cause | Action |
|---|---|---|---|
| `0x81` | Error sensor 1 | Sensor break/short T1(L) or T3(R) | Check or replace sensor |
| `0x82` | Error sensor 2 | Sensor break/short T2(L) or T4(R) | Check or replace sensor |
| `0x83` | Error sensor 3 | Sensor break/short T3(L) or T1(R) | Check or replace sensor |
| `0x84` | Error sensor 4 | Sensor break/short T4(L) or T2(R) | Check or replace sensor |
| `0x85` | Supply air temperature too low | Min supply air temp < setpoint | Clears when temp > setpoint + 1K |
| `0x86` | Intake air temperature too low | Intake air temp < setpoint for >30 min | Clears when temp > setpoint; recheck after 1h |
| `0x87` | Error fan 1 Hall | Supply fan speed not reporting (L) / Exhaust (R) | Manual fan speed adjustment |
| `0x88` | Error fan 2 Hall | Exhaust fan speed not reporting (L) / Supply (R) | Manual fan speed adjustment |
| `0x89` | Error by-pass | No end position, bypass not working | Test bypass (open/close) |
| `0xC0` | BUS version incompatible | Software versions not compatible | Replace/update software |
| `0xC1` | Too many devices connected | Too many components on the bus | Remove surplus components |
| `0xC2` | Fan slave not connected | Lack of BUS communication | Connect fan slave |
| `0xC3` | Communication error fan slave | Lack of BUS communication | Check BUS communication |
| `0xC4` | Communication error general | Lack of BUS communication | Power cycle, then restart |
| `0xC5` | Communication error heater battery | Lack of BUS communication | Check BUS communication |
| `0xC6` | Comm. error GHE flap | Lack of BUS communication | Check BUS communication |
| `0xC7` | Communication error defroster | Lack of BUS communication | Check BUS communication |
| `0xC8` | Heating does not switch off | Error BUS thermostat | Replace BUS thermostat |
| `0xC9` | General control unit error | Lack of BUS comm with control unit | Check BUS communication |

The ASCII message after the code byte identifies the specific device (e.g., `0xC7` + "Touch TFT 1" = defroster communication error reported by TFT control unit #1).

On error: fans are switched off and bypass closes.

### LED Control Panel Error Codes (Binary)

The LED panel displays errors as a binary pattern across LEDs L1-L7 (active LEDs marked with X):

| L1 | L2 | L3 | L4 | L5 | L6 | L7 | Error |
|---|---|---|---|---|---|---|---|
| X | | X | | | | | Supply air temperature too low |
| X | | | X | | | | Error bypass |
| X | X | | X | X | | X | BUS version incompatible |
| | | X | X | X | | X | Too many devices connected |
| X | | X | X | X | | X | Fan slave not connected |
| | X | X | X | X | | X | Communication error fan slave |
| X | X | X | X | X | | X | Communication error defroster |
| | | | | | X | X | Communication error heater battery |
| X | | | | | X | X | Communication error GHE flap |
| | X | | | | X | X | Communication error general |
| X | X | | | | X | X | Heating does not switch off |
| | X | | X | X | | | General control unit error |

Additional LED signals:
- **L1+L7 lit**: No external release — fans off
- **L8 lit**: Supply and extract air mode active
- **L8 blinks**: Error sensor — fans off, bypass closes
- **L8 flashes**: Standby mode
- **L9 lit**: Boost ventilation mode
- **L10 lit**: Filter run-time expired
- **L10 flashes**: Filter remaining run-time <= 10 days
- **L11 lit**: Extract air mode
- **L11 blinks**: Error fan 1 Hall — fans off, bypass closes
- **L12 lit**: Supply air mode
- **L12 blinks**: Error fan 2 Hall — fans off, bypass closes

## Open Questions / To Be Done

- Register `0x44`: Always value `0x5C` (92). Sent frequently with temperatures. Status/flag bitmask?
- Register `0x25`: Always value `0x36` (54). Constant across reboots. Hardware config ID?
- External release state register (bridged/TFT locked)
- Device type identification register (Novus 300/450, F variant)
- Construction type register (Vertical/Horizontal, Left/Right)
- Digital output DIO2 state register
- Filter reset command register
- "Supplementary heating released" toggle (separate from heating type?)
- Multi-panel configuration register ("number of this control panel")
- Key lock state register
