# Troubleshooting Guide

## Common Issues and Solutions

### Connection Issues

#### Serial Port Not Found
**Symptom**: Error about serial port not found or permission denied

**Solutions**:
1. Check that USB device is connected:
   ```bash
   ls -la /dev/ttyUSB*
   ```

2. Verify correct port:
   ```bash
   dmesg | grep -i usb
   ```

3. Update port in `.env`:
   ```bash
   SERIAL_PORT=/dev/ttyUSB1
   ```

4. Check permissions (Linux):
   ```bash
   sudo usermod -a -G dialout $USER
   # Then log out and back in
   ```

---

#### MQTT Broker Connection Failed
**Symptom**: Error connecting to MQTT broker or timeout

**Solutions**:
1. Verify broker is running:
   ```bash
   ping your-mqtt-broker-ip
   telnet your-mqtt-broker-ip 1883
   ```

2. Check firewall:
   ```bash
   sudo ufw allow 1883
   ```

3. Verify credentials:
   ```bash
   mosquitto_sub -h your-broker -u username -P password -t "#"
   ```

4. Check broker logs:
   ```bash
   journalctl -u mosquitto -f
   ```

---

#### Bus Communication Timeout
**Symptom**: No data received from device

**Solutions**:
1. Verify RS485 bus wiring (check pin assignments on the Novus connector)
2. Confirm device is powered on
3. Check baud rate matches (default: 9600)
4. Try running with debug logging:
   ```bash
   LOG_LEVEL=debug cargo run
   ```

---

### Configuration Issues

#### Missing Configuration File
**Symptom**: Error about `config/device.yaml` or `config/registers.yaml` not found.

The app loads two YAML files — `device.yaml` (unit identity) and `registers.yaml`
(register map, auto-discovered next to `device.yaml`).

**Solutions**:
1. Verify both files exist:
   ```bash
   ls -la config/device.yaml config/registers.yaml
   ```

2. Set custom paths via env vars:
   ```bash
   NOVUS_CONFIG=path/to/device.yaml NOVUS_REGISTERS=path/to/registers.yaml cargo run
   ```

3. Validate configuration:
   ```bash
   cargo run -- validate-config
   ```

---

#### Invalid YAML Configuration
**Symptom**: YAML parsing error on startup

**Solutions**:
1. Check YAML syntax (use spaces, not tabs; verify indentation)
2. Run config validation:
   ```bash
   cargo run -- validate-config
   ```

---

### Data Issues

#### No Data Published to MQTT
**Symptom**: MQTT topics empty, no sensor data appearing

**Solutions**:
1. Enable debug logging:
   ```bash
   LOG_LEVEL=debug cargo run
   ```

2. Monitor MQTT topics:
   ```bash
   mosquitto_sub -h your-broker -u user -P pass -t "novus2mqtt/#" -v
   ```

3. Verify bus communication in logs (look for received frame messages)

4. Record bus traffic for analysis:
   ```bash
   BUS_DUMP_FILE=bus_dump.bin cargo run
   ```

---

#### Home Assistant Not Discovering Devices
**Symptom**: Entities not appearing in Home Assistant

**Solutions**:
1. Verify discovery is enabled in `.env`:
   ```
   HASS_DISCOVERY=true
   HASS_DISCOVERY_PREFIX=homeassistant
   ```

2. Monitor discovery messages:
   ```bash
   mosquitto_sub -h your-broker -t "homeassistant/#" -v
   ```

3. Verify HA's MQTT integration discovery prefix matches `HASS_DISCOVERY_PREFIX`

4. Restart Home Assistant after the bridge has published discovery messages

5. Remove stale retained discovery topics if ghost entities remain:
   ```bash
   mosquitto_pub -h your-broker -t 'homeassistant/sensor/device_id/entity/config' -n -r
   ```

---

### Build Issues

#### Compilation Errors
**Symptom**: `cargo build` fails

**Solutions**:
1. Update Rust toolchain:
   ```bash
   rustup update stable
   ```

2. Clean and rebuild:
   ```bash
   cargo clean && cargo build
   ```

3. Check that all system dependencies are installed (e.g., `libudev-dev` on Debian/Ubuntu for serial port support)

---

### Performance Issues

#### High CPU Usage
**Solutions**:
1. Reduce logging level:
   ```bash
   LOG_LEVEL=warn cargo run
   ```

2. Check diagnostics topic for message rate:
   ```bash
   mosquitto_sub -h your-broker -t "novus2mqtt/+/diagnostics"
   ```

---

## Debugging Tips

### Enable Debug Logging
```bash
LOG_LEVEL=debug cargo run 2>&1 | tee debug.log
```

### Monitor All MQTT Topics
```bash
mosquitto_sub -h your-broker -u user -P pass -t "#" -v
```

### Validate Configuration
```bash
cargo run -- validate-config
```

### Check Health
```bash
cargo run -- health-check
```

### Record Bus Traffic
```bash
BUS_DUMP_FILE=dump.bin cargo run
```

---

## Getting Help

1. Review [ARCHITECTURE.md](ARCHITECTURE.md) for system design
2. Check [CHANGELOG.md](CHANGELOG.md) for recent changes
3. Run `cargo run -- validate-config` to check configuration
4. Run `cargo run -- health-check` to verify connectivity
5. Open an issue on GitHub with:
   - Error message
   - Steps to reproduce
   - Logs (with credentials removed)
   - Environment details (OS, Rust version)
