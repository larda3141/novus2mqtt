/// Bus protocol constants and CRC functions for the Paul Novus serial bus.
/// Valid bus device addresses.
pub const BUS_DEVICES: &[u8] = &[
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0xFF,
];

/// Valid bus command bytes.
pub const BUS_COMMANDS: &[u8] = &[0x00, 0x80, 0x81, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88];

/// Commands after which a pending write may be executed.
pub const BUS_WRITE_AFTER_COMMANDS: &[u8] = &[0x84, 0x87];

/// Header size: first_byte + device + cmd + length + checksum(2).
pub const MSG_HEADER_SIZE: usize = 6;

/// Maximum message size to prevent memory issues.
pub const MSG_MAX_SIZE: usize = 256;

/// Timeout for incomplete messages (seconds).
pub const MSG_TIMEOUT_SECS: f64 = 1.0;

/// CRC16-CCITT as used by the Novus bus protocol.
///
/// This is the exact algorithm from the Python implementation —
/// **not** the standard CCITT polynomial. The Novus bus uses a
/// custom byte-wise computation.
pub fn crc16_ccitt(crc_init: u16, data: &[u8]) -> u16 {
    let mut msb = (crc_init >> 8) as u8;
    let mut lsb = (crc_init & 0xFF) as u8;
    for &c in data {
        let mut x = c ^ msb;
        x ^= x >> 4;
        msb = lsb ^ (x >> 3) ^ (x << 4);
        lsb = x ^ (x << 5);
    }
    ((msb as u16) << 8) + lsb as u16
}

/// Compute the Novus two-byte checksum (LSB first, then MSB).
pub fn novus_checksum(data: &[u8]) -> [u8; 2] {
    let crc = crc16_ccitt(0, data);
    [(crc & 0xFF) as u8, (crc >> 8) as u8]
}

/// Validate a complete Novus bus message checksum.
///
/// The checksum bytes are at positions 4..6 and cover bytes 0..4 + 6..end.
pub fn novus_validate_checksum(data: &[u8]) -> bool {
    if data.len() < MSG_HEADER_SIZE {
        return false;
    }
    let ref_crc = &data[4..6];
    let mut check_data = Vec::with_capacity(data.len() - 2);
    check_data.extend_from_slice(&data[0..4]);
    check_data.extend_from_slice(&data[6..]);
    let computed = novus_checksum(&check_data);
    ref_crc == computed
}

/// Build a complete bus message matching the original Python format.
///
/// Format: `address(2) + command + length + checksum(2) + data`
/// Checksum uses the Novus custom CRC over `header + data`.
pub fn build_bus_message(address: &[u8; 2], command: u8, data: &[u8]) -> Vec<u8> {
    let mut length = data.len() as u8;
    if command == 0x87 {
        length = length.wrapping_add(0x80);
    }

    let header: Vec<u8> = vec![address[0], address[1], command, length];
    let checksum = novus_checksum(&[&header[..], data].concat());

    let mut message = Vec::with_capacity(6 + data.len());
    message.extend_from_slice(&header);
    message.push(checksum[0]); // CRC LSB at byte 4
    message.push(checksum[1]); // CRC MSB at byte 5
    message.extend_from_slice(data);

    message
}

/// Human-readable command name for logging.
#[allow(dead_code)]
pub fn command_name(cmd: u8) -> &'static str {
    match cmd {
        0x00 => "clock",
        0x80 => "join_req",
        0x81 => "join_resp",
        0x83 => "dump",
        0x84 => "ping_req",
        0x85 => "mem_write",
        0x86 => "update_req",
        0x87 => "checksum",
        0x88 => "checksum_err",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc16_ccitt_empty() {
        assert_eq!(crc16_ccitt(0, &[]), 0);
    }

    #[test]
    fn test_novus_checksum_roundtrip() {
        let data = vec![0x01, 0x02, 0x83, 0x00];
        let cs = novus_checksum(&data);
        // Build a message with the checksum inserted at positions 4..6
        let mut msg = data.clone();
        msg.push(cs[0]);
        msg.push(cs[1]);
        assert!(novus_validate_checksum(&msg));
    }

    #[test]
    fn test_novus_validate_checksum_bad() {
        let msg = vec![0x01, 0x02, 0x83, 0x00, 0x00, 0x00];
        // Unlikely to match the real checksum
        // (unless by coincidence, which is fine for a smoke test)
        let expected = novus_checksum(&[0x01, 0x02, 0x83, 0x00]);
        if msg[4..6] != expected {
            assert!(!novus_validate_checksum(&msg));
        }
    }

    #[test]
    fn test_build_bus_message_length() {
        let msg = build_bus_message(&[0x01, 0x01], 0x86, &[]);
        // 2 (addr) + 1 (cmd) + 1 (len) + 2 (crc) + 0 (data) = 6
        assert_eq!(msg.len(), 6);
        assert_eq!(msg[0], 0x01); // addr[0]
        assert_eq!(msg[1], 0x01); // addr[1]
        assert_eq!(msg[2], 0x86); // command
        assert_eq!(msg[3], 0); // length byte
                               // bytes 4-5 are checksum
                               // Validate the checksum is correct
        assert!(novus_validate_checksum(&msg));
    }

    #[test]
    fn test_command_name() {
        assert_eq!(command_name(0x85), "mem_write");
        assert_eq!(command_name(0xFF), "unknown");
    }

    #[test]
    fn test_crc16_known_vector() {
        // Known bus message from capture: dump request 01 00 83 00 7f 38
        // CRC input: [0x01, 0x00, 0x83, 0x00], expected checksum: [0x7F, 0x38]
        let data = [0x01, 0x00, 0x83, 0x00];
        let cs = novus_checksum(&data);
        assert_eq!(cs, [0x7F, 0x38]);
    }

    #[test]
    fn test_build_bus_message_with_data() {
        // Build a write command for register 0x28 (vent level) with value 0x06 (Auto)
        let msg = build_bus_message(&[0x01, 0x00], 0x85, &[0x28, 0x00, 0x06]);
        assert_eq!(msg.len(), 9); // 6 header + 3 data
        assert_eq!(msg[2], 0x85); // command
        assert_eq!(msg[3], 3); // length
        assert_eq!(msg[6], 0x28); // register
        assert_eq!(msg[7], 0x00); // separator
        assert_eq!(msg[8], 0x06); // value
        assert!(novus_validate_checksum(&msg));
    }

    #[test]
    fn test_build_bus_message_0x87_length_flag() {
        // Command 0x87 adds 0x80 to the length byte
        let msg = build_bus_message(&[0x01, 0x01], 0x87, &[0xAA, 0xBB, 0xCC]);
        assert_eq!(msg[3], 0x83); // 3 + 0x80 = 0x83
        assert!(novus_validate_checksum(&msg));
    }

    #[test]
    fn test_validate_checksum_rejects_corruption() {
        let mut msg = build_bus_message(&[0x01, 0x00], 0x85, &[0x28, 0x00, 0x06]);
        assert!(novus_validate_checksum(&msg));
        // Corrupt one data byte
        msg[8] = 0x07;
        assert!(!novus_validate_checksum(&msg));
    }

    #[test]
    fn test_validate_checksum_too_short() {
        assert!(!novus_validate_checksum(&[0x01, 0x02]));
        assert!(!novus_validate_checksum(&[]));
    }
}
