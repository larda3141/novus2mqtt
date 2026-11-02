# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 2025.x.x (Rust) | Current |

We recommend always using the latest version.

## Reporting a Vulnerability

Please **do not** open public GitHub issues for security vulnerabilities.

**Preferred channel:** [Open a GitHub Security Advisory](https://github.com/larda3141/novus2mqtt/security/advisories/new). This is private and structured (CVE assignment, fix tracking, coordinated disclosure).

Include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

We will:
1. Acknowledge your report within 7 days
2. Investigate and validate the issue
3. Develop and test a fix
4. Release a patched version
5. Credit you in the release notes (optional)

## Security Best Practices

When running novus2mqtt:

1. **Secure MQTT Broker**:
   - Use strong username/password
   - Consider TLS encryption
   - Restrict network access to MQTT port

2. **Environment Variables**: Never commit `.env` files with credentials. The
   `.env` file should be owner-readable only:
   ```bash
   chmod 600 .env
   ```

3. **Serial Port Access**:
   - Restrict `/dev/ttyUSB0` permissions
   - Only authorized users should access the serial device

4. **Logging**:
   - Credentials are not logged
   - Review logs for suspicious activity

## Dependency Security

This project uses Rust crates audited via `cargo audit`. Key dependencies:

- `rumqttc` - MQTT client
- `tokio-serial` - Serial port communication
- `serde_yaml_ng` - Configuration parsing (active fork of unmaintained `serde_yaml`)
- `sysinfo` - System metrics
- `tokio` - Async runtime

Check for known vulnerabilities:
```bash
cargo install cargo-audit
cargo audit
```

## Known Limitations

- Serial communication uses CRC16 for integrity, not cryptographic signatures
- MQTT traffic should be protected by broker-level TLS
- Register values are not encrypted on the RS485 bus

---

**Last updated**: 2026-04-17
