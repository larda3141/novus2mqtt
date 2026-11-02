#!/usr/bin/env python3
"""
Optimize device.yaml: strip unused fields, ensure proper conversion info,
and generate correct Home Assistant MQTT discovery config per HA best practices.

HA MQTT Discovery best practices applied:
- sensor: state_class=measurement for numeric, device_class for known types,
  suggested_display_precision for decimals, expire_after for liveness
- number: min/max/step/unit_of_measurement/mode, command_template
- select: options list (required), command_template
- Proper value_template for JSON payloads
- entity_category: diagnostic for system/version/memory, config for settings
"""

import yaml
import sys
import copy

# HA device_class mapping based on topic keywords and payload type
TOPIC_DEVICE_CLASS = {
    'temperature': {'device_class': 'temperature', 'state_class': 'measurement', 'unit': '°C', 'precision': 1},
    'humidity': {'device_class': 'humidity', 'state_class': 'measurement', 'unit': '%', 'precision': 1},
    'pressure': {'device_class': 'pressure', 'state_class': 'measurement'},
    'operating_hours': {'device_class': 'duration', 'state_class': 'total_increasing', 'unit': 's'},
}

# Topics that should be diagnostic category
DIAGNOSTIC_TOKENS = ['version', 'memory', 'mem_0x', 'error', 'message', 'bus_version', 'unknown']
# Topics that should NOT be diagnostic even if they match above tokens
DIAGNOSTIC_EXCLUDES = ['temperature', 'fan_speed', 'humidity', 'pressure', 'bypass',
                       'ventilation', 'heating', 'defroster', 'frost', 'filter',
                       'operating_hours', 'sensor']
# Topics that should be config category
CONFIG_TOKENS = ['setting', 'language', 'dimming', 'threshold', 'hysteresis']

# Icon mapping
ICON_MAP = {
    'temperature': 'mdi:thermometer',
    'humidity': 'mdi:water-percent',
    'frost': 'mdi:snowflake',
    'defrost': 'mdi:snowflake',
    'bypass': 'mdi:pipe',
    'version': 'mdi:information-outline',
    'error': 'mdi:alert-circle-outline',
    'message': 'mdi:alert-circle-outline',
    'fan_speed': 'mdi:fan',
    'ventilation': 'mdi:air-filter',
    'filter': 'mdi:air-filter',
    'heating': 'mdi:radiator',
    'operating_hours': 'mdi:clock-outline',
    'summer': 'mdi:weather-sunny',
    'language': 'mdi:translate',
    'state': 'mdi:power',
    'imbalance': 'mdi:scale-unbalanced',
}


def infer_entity_category(topic, name, ha_type):
    """Infer entity_category from topic/name."""
    t = (topic + ' ' + name).lower()
    # Check exclusions first - primary entities should never be diagnostic
    for token in DIAGNOSTIC_EXCLUDES:
        if token in t:
            return None
    for token in DIAGNOSTIC_TOKENS:
        if token in t:
            return 'diagnostic'
    for token in CONFIG_TOKENS:
        if token in t:
            return 'config'
    return None


def infer_icon(topic):
    """Infer icon from topic keywords."""
    t = topic.lower()
    for keyword, icon in ICON_MAP.items():
        if keyword in t:
            return icon
    return None


def infer_device_class_info(topic, payload):
    """Infer device_class, state_class, unit from topic and payload type."""
    t = topic.lower()
    for keyword, info in TOPIC_DEVICE_CLASS.items():
        if keyword in t:
            return info
    # Numeric sensors with unit get measurement state_class
    ptype = payload.get('type', '')
    if ptype in ('int', 'byte') and payload.get('unit_of_measurement'):
        return {'state_class': 'measurement'}
    if ptype == 'counter':
        return {'device_class': 'duration', 'state_class': 'total_increasing', 'unit': 's'}
    return {}


def build_value_template(payload, ha_type):
    """Build the correct value_template for this entity type."""
    ptype = payload.get('type', 'byte')

    if ha_type == 'select':
        # Select needs the converted string value
        return "{{ value_json.converted | default(value_json.hex) }}"

    if ptype in ('char', 'date', 'time'):
        return "{{ value_json.converted | default(value_json.hex) }}"

    if ptype in ('int', 'byte') and payload.get('decimals', 0) > 0:
        # Numeric with decimals - HA needs a number
        return "{{ value_json.converted | float }}"

    if ptype in ('int', 'byte') and not payload.get('options'):
        # Plain numeric
        return "{{ value_json.converted | default(value_json.hex) }}"

    if ptype == 'counter':
        return "{{ value_json.converted | int }}"

    # Default
    return "{{ value_json.converted | default(value_json.hex) }}"


def build_command_template(payload, ha_type):
    """Build command_template for writable entities."""
    if ha_type == 'number':
        return "{{ '%02x' % (value | int) }}"
    if ha_type == 'select':
        # Select sends the option value directly - handled by payload encode
        return None
    return None


def optimize_entity(entity, section):
    """Optimize an entity entry with proper HA discovery config."""
    optimized = {}

    addr = entity['address']
    optimized['address'] = addr

    # Payload config - keep all conversion-relevant fields
    if 'payload' in entity:
        p = entity['payload']
        payload = {}
        for key in ['length', 'type', 'decimals', 'signed', 'options',
                     'unit_of_measurement', 'min', 'max', 'step', 'command',
                     'class']:
            if key in p and p[key] is not None:
                payload[key] = p[key]
        if payload:
            optimized['payload'] = payload
    else:
        payload = {}

    topic = entity.get('topic', '')
    name = entity.get('name', addr)
    unique_id = entity.get('unique_id', '')

    optimized['topic'] = topic

    # Set unknown memory registers to type: unknown for multi-format conversion
    if (name.lower() == 'unknown' or 'memory/' in topic.lower()) and payload.get('type') == 'byte':
        payload['type'] = 'unknown'
        optimized['payload'] = payload

    # Publish flag - only include if false (default is true)
    if entity.get('publish') is False:
        optimized['publish'] = False

    # Expose to Home Assistant - hide memory/unknown registers by default
    is_memory = 'mem_0x' in (unique_id or '') or 'memory/' in topic.lower()
    is_unknown = name.lower() in ('unknown',)
    if entity.get('expose_ha') is not None:
        if not entity['expose_ha']:
            optimized['expose_ha'] = False
    elif is_memory or is_unknown:
        optimized['expose_ha'] = False

    optimized['name'] = name
    if unique_id:
        optimized['unique_id'] = unique_id

    # Operations: RO (read-only) or RW (read-write)
    # Derive from section or existing command_topic
    if section == 'controls' or entity.get('command_topic'):
        optimized['operations'] = 'RW'
    else:
        optimized['operations'] = 'RO'

    # Command topic for writable entities
    if entity.get('command_topic'):
        optimized['command_topic'] = entity['command_topic']

    # Build Home Assistant discovery config
    ha = entity.get('home_assistant', {})
    ha_type = ha.get('type', 'sensor')
    existing_config = ha.get('config', {}) if ha.get('config') else {}

    # Fix mistyped entities
    ptype = payload.get('type', 'byte')

    # Date/time should be sensor, not number (they're writable via command topic)
    if ptype in ('date', 'time'):
        ha_type = 'sensor'

    # Counter should be sensor, not number
    if ptype == 'counter':
        ha_type = 'sensor'

    # Select without options -> number or sensor
    if ha_type == 'select' and not payload.get('options'):
        if ptype in ('int', 'byte') and payload.get('min') is not None:
            ha_type = 'number'
        elif ptype in ('int',) and payload.get('signed') is not None:
            ha_type = 'number'
        else:
            ha_type = 'sensor'

    ha_config = {}

    # === Value template (always needed) ===
    val_tpl = build_value_template(payload, ha_type)
    ha_config['val_tpl'] = val_tpl

    # === Entity category ===
    # Use inference (which checks excludes), not existing config which may be wrong
    cat = infer_entity_category(topic, name, ha_type)
    if cat:
        ha_config['entity_category'] = cat

    # === Device class & state class (sensors) ===
    if ha_type == 'sensor':
        dc_info = infer_device_class_info(topic, payload)
        if dc_info.get('device_class'):
            ha_config['dev_cla'] = dc_info['device_class']
        if dc_info.get('state_class'):
            ha_config['stat_cla'] = dc_info['state_class']
        if dc_info.get('precision') is not None:
            ha_config['sug_dsp_prc'] = dc_info['precision']

        # Unit of measurement
        unit = payload.get('unit_of_measurement') or dc_info.get('unit')
        if unit:
            ha_config['unit_of_meas'] = unit

        # Enum sensors with options
        ptype = payload.get('type', '')
        if payload.get('options') and not ha_config.get('dev_cla'):
            ha_config['dev_cla'] = 'enum'
            ha_config['options'] = list(payload['options'].values())

        # Expire after for liveness detection
        ha_config['expire_after'] = 600

    # === Select entities ===
    elif ha_type == 'select':
        if payload.get('options'):
            ha_config['options'] = list(payload['options'].values())
        # Command topic
        cmd_topic = entity.get('command_topic') or f"{topic}/set"
        ha_config['cmd_t'] = f"~/{cmd_topic}"
        ha_config['cmd_tpl'] = "{{ value }}"

    # === Number entities ===
    elif ha_type == 'number':
        if payload.get('min') is not None:
            ha_config['min'] = payload['min']
        if payload.get('max') is not None:
            ha_config['max'] = payload['max']
        ha_config['step'] = payload.get('step', 1)
        unit = payload.get('unit_of_measurement')
        if unit:
            ha_config['unit_of_meas'] = unit
        ha_config['mode'] = 'slider' if payload.get('max', 0) - payload.get('min', 0) <= 100 else 'box'
        # Command topic
        cmd_topic = entity.get('command_topic') or f"{topic}/set"
        ha_config['cmd_t'] = f"~/{cmd_topic}"

    # === Icon ===
    icon = existing_config.get('icon') or infer_icon(topic)
    if icon:
        ha_config['icon'] = icon

    # Preserve any custom config that wasn't auto-generated
    for key in ['expire_after', 'force_update', 'enabled_by_default', 'qos',
                'payload_on', 'payload_off', 'payload_press']:
        if key in existing_config and key not in ha_config:
            ha_config[key] = existing_config[key]

    optimized['home_assistant'] = {
        'type': ha_type,
        'config': ha_config,
    }

    return optimized


def main():
    input_path = sys.argv[1] if len(sys.argv) > 1 else 'config/device.yaml'
    output_path = sys.argv[2] if len(sys.argv) > 2 else 'config/device.optimized.yaml'

    with open(input_path) as f:
        data = yaml.safe_load(f)

    optimized = {}
    optimized['device'] = data['device']

    if 'bus' in data:
        bus = data['bus']
        bus_opt = {}
        for key in ['port', 'baudrate', 'timeout', 'sw_version', 'commands',
                     'available_device_addresses']:
            if key in bus:
                bus_opt[key] = bus[key]
        optimized['bus'] = bus_opt

    for section in ['sensors', 'controls', 'binary_sensors']:
        if section in data and data[section]:
            optimized[section] = [optimize_entity(e, section) for e in data[section]]

    with open(output_path, 'w') as f:
        yaml.dump(optimized, f, default_flow_style=False, allow_unicode=True, sort_keys=False, width=120)

    # Stats
    orig_lines = sum(1 for _ in open(input_path))
    new_lines = sum(1 for _ in open(output_path))

    entities = {}
    for section in ['sensors', 'controls', 'binary_sensors']:
        for e in (optimized.get(section) or []):
            ha_type = e.get('home_assistant', {}).get('type', 'unknown')
            entities[ha_type] = entities.get(ha_type, 0) + 1

    total = sum(entities.values())
    print(f"Entities: {total} ({', '.join(f'{v} {k}' for k, v in sorted(entities.items()))})")
    print(f"Lines: {orig_lines} -> {new_lines} ({100 - new_lines*100//orig_lines}% reduction)")

    # Verify conversion info completeness
    issues = 0
    for section in ['sensors', 'controls', 'binary_sensors']:
        for e in (optimized.get(section) or []):
            p = e.get('payload', {})
            if not p.get('type'):
                print(f"  WARN: {e['address']} missing payload.type")
                issues += 1
            if not p.get('length'):
                print(f"  WARN: {e['address']} missing payload.length")
                issues += 1
            ha = e.get('home_assistant', {})
            if ha.get('type') == 'select' and not ha.get('config', {}).get('options'):
                print(f"  WARN: {e['address']} select without options")
                issues += 1

    print(f"Issues: {issues}")
    print(f"Output: {output_path}")


if __name__ == '__main__':
    main()
