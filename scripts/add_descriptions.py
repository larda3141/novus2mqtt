#!/usr/bin/env python3
"""Add descriptions to device.yaml from the backup file and bus dump findings."""

import yaml

# Findings from bus dump analysis for unknown/undocumented registers
BUS_DUMP_FINDINGS = {
    "0x08": "Write-ping address. Value observed: 0x00-0x03. Changes during boot sequence.",
    "0x0e": "TFT Control Unit #1 status. Write-ping address. Value: 0x00 when device not present.",
    "0x0f": "Unknown. Value: 0x00.",
    "0x11": "LED Control Unit related. Write-ping address. Value: 0x00 when device not present.",
    "0x12": "Unknown. Value: 0x00.",
    "0x14": "Other Device #1 related. Write-ping address. Value: 0x00 when device not present.",
    "0x15": "Unknown. Value: 0x00.",
    "0x17": "Other Device #2 related. Write-ping address. Value: 0x00 when device not present.",
    "0x1a": "Slave device health status. Write-ping address. Value: 0x0D (13) when healthy/connected, 0x00 when missing/disconnected.",
    "0x1d": "PTC Defroster health status. Write-ping address. Value: 0x01-0x03 when active, 0x00 when missing.",
    "0x20": "Other Device #3 status. Write-ping address. Value: 0x00 when device not present.",
    "0x23": "Other Device #4 status. Write-ping address. Value: 0x00 when device not present.",
    "0x25": "Hardware config or checksum. Constant value 0x36 (54). Ping also in standby. Never changes across reboots.",
    "0x2c": "Active maximum fan speed (mirror). Value: 100 matches fan_speed_purge (0x30). Updated when waking from standby.",
    "0x2d": "Active minimum fan speed (mirror). Value: 17 matches fan_speed_reduced (0x2e). Updated when waking from standby.",
    "0x40": "Filter-related. Value: 3. Appears in filter register block near 0x3d-0x3f.",
    "0x44": "System status bitmask. Value: 0x5C (01011100). Sent every ~1s alongside temperatures. Write-ping address. Bits may indicate: bypass state, defroster state, fan running, system mode.",
    "0x45": "Possibly sensor reading. Value: 155 (if decimals=1: 15.5 C). Near bypass/frost protection registers.",
    "0x46": "Possibly fan speed reference. Value: 100 (100%). Near bypass/frost protection registers.",
    "0x48": "Hysteresis setting. Value: 5 (matches bypass hysteresis 0x43=0.5K). Between frost protection thresholds.",
    "0x57": "Sensor parameter. Value: 10. Near sensor output parameters block.",
    "0x58": "Sensor parameter. Value: 6. Near sensor output parameters block.",
    "0x59": "Sensor parameter. Value: 5. Near sensor output parameters block.",
    "0x5b": "Fan speed reference (mirror). Value: 0x0032 (50). Matches fan_speed_nominal setting (0x2f=50%). 2-byte register.",
    "0x5c": "Post-heating parameter. Value: 5. Near post-heating settings block.",
    "0x5d": "Post-heating parameter. Value: 40. Near post-heating settings block.",
    "0x5e": "GHE-flap related. Value: 0x00. Near GHE-flap settings.",
    "0x61": "Bypass parameter. Value: 5. Near bypass test register (0x62).",
    "0x7c": "Frost protection parameter. Value: 5. Near frost protection mode (0x7b). Possibly hysteresis.",
    "0x7d": "Temperature sensor reference. Value: 0x81 (129) = register address of T2 sensor. Possibly 'which sensor for frost protection input'.",
    "0x7e": "Temperature sensor reference. Value: 0x82 (130) = register address of T1 sensor. Possibly 'which sensor for frost protection output'.",
    "0x86": "Fan running flag. Value: 1 when fans active, adjacent to fan operating hours (0x85). Possibly counter enable flag.",
    "0x87": "Current nominal fan speed reference. Value: 50 matches fan_speed_nominal (0x2f=50%). May be the active target speed.",
    "0x8e": "Unknown. Value: 0x00. After error message block. Possibly error count or flag.",
    "0x91": "TFT Control Unit #2 status. Value: 0x00 when not present.",
    "0x94": "TFT Control Unit #3 status. Value: 0x00 when not present.",
}


def main():
    # Load current config
    with open('config/device.yaml') as f:
        current = yaml.safe_load(f)

    # Load backup with descriptions
    with open('config/device.yaml.bak') as f:
        backup = yaml.safe_load(f)

    # Build description map from backup
    desc_map = {}
    for section in ['sensors', 'controls', 'binary_sensors']:
        for e in (backup.get(section) or []):
            addr = e['address'].lower()
            desc = e.get('description_i18n', {})
            if desc and desc.get('en'):
                desc_map[addr] = desc['en']

    # Updated names for registers we now understand
    NAME_UPDATES = {
        "0x0e": "TFT Control Unit #1 Status",
        "0x11": "LED Control Unit Status",
        "0x14": "Other Device #1 Status",
        "0x17": "Other Device #2 Status",
        "0x1a": "Slave Health Status",
        "0x1d": "PTC Defroster Health Status",
        "0x20": "Other Device #3 Status",
        "0x23": "Other Device #4 Status",
        "0x2c": "Active Max Fan Speed (mirror)",
        "0x2d": "Active Min Fan Speed (mirror)",
        "0x44": "System Status Bitmask",
        "0x5b": "Fan Speed Nominal (mirror)",
        "0x7d": "Frost Protection Input Sensor Ref",
        "0x7e": "Frost Protection Output Sensor Ref",
        "0x86": "Fan Running Flag",
        "0x87": "Current Nominal Fan Speed Ref",
    }

    # Add descriptions and update names
    updated = 0
    for section in ['sensors', 'controls', 'binary_sensors']:
        for e in (current.get(section) or []):
            addr = e['address'].lower()

            # Update names for registers we now understand
            if addr in NAME_UPDATES and e.get('name', '') == 'Unknown':
                e['name'] = NAME_UPDATES[addr]

            # Get description from backup or findings
            desc = desc_map.get(addr, '')

            # For unknowns or empty descriptions, use bus dump findings
            if addr in BUS_DUMP_FINDINGS:
                if not desc or desc in ('Unknown', 'Unbekannt', ''):
                    desc = BUS_DUMP_FINDINGS[addr]
                else:
                    # Append findings to existing description
                    desc = desc + " | Bus analysis: " + BUS_DUMP_FINDINGS[addr]

            if desc:
                e['description'] = desc
                updated += 1

    # Write back
    with open('config/device.yaml', 'w') as f:
        yaml.dump(current, f, default_flow_style=False, allow_unicode=True, sort_keys=False, width=120)

    print(f"Updated {updated} entities with descriptions")


if __name__ == '__main__':
    main()
