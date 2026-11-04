use std::time::Instant;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tracing::{debug, error, info, warn};

use crate::protocol::{
    command_name, novus_validate_checksum, BUS_COMMANDS, BUS_DEVICES, MSG_HEADER_SIZE,
    MSG_MAX_SIZE, MSG_TIMEOUT_SECS,
};

/// Upper bound for a single serial read in the main loop.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

/// Parser state machine states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusParserState {
    WaitingHeader,
    ReadingData,
}

/// State-machine parser for Paul Novus bus messages.
pub struct BusMessageParser {
    buffer: Vec<u8>,
    state: BusParserState,
    expected_length: usize,
    message_start_time: Instant,
}

impl BusMessageParser {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            state: BusParserState::WaitingHeader,
            expected_length: 0,
            message_start_time: Instant::now(),
        }
    }

    /// Feed incoming bytes and return any complete, validated messages.
    pub fn add_data(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        let mut complete_messages = Vec::new();

        for &byte in data {
            self.buffer.push(byte);

            if self.buffer.len() > MSG_MAX_SIZE {
                warn!("Bus parser: buffer overflow, resetting");
                self.reset();
                continue;
            }

            if self.state == BusParserState::WaitingHeader && self.parse_header() {
                self.state = BusParserState::ReadingData;
                self.message_start_time = Instant::now();
            }

            if self.state == BusParserState::ReadingData {
                let total_length = MSG_HEADER_SIZE + self.expected_length;
                if self.buffer.len() >= total_length {
                    if self.validate_message() {
                        complete_messages.push(self.buffer.clone());
                        self.reset();
                    } else {
                        self.handle_invalid_message();
                    }
                } else if self.message_start_time.elapsed().as_secs_f64() > MSG_TIMEOUT_SECS {
                    debug!("Bus parser: message timeout, resetting");
                    self.reset();
                }
            }
        }

        complete_messages
    }

    fn parse_header(&mut self) -> bool {
        if self.buffer.len() < MSG_HEADER_SIZE {
            return false;
        }

        // Validate first byte
        if self.buffer[0] != 0x00 && self.buffer[0] != 0x01 {
            self.buffer.remove(0);
            return false;
        }

        // Validate device
        if !BUS_DEVICES.contains(&self.buffer[1]) {
            self.skip_potential_message();
            return false;
        }

        // Validate command
        if !BUS_COMMANDS.contains(&self.buffer[2]) {
            self.skip_potential_message();
            return false;
        }

        // Calculate expected data length
        let length_byte = self.buffer[3];
        self.expected_length = if length_byte >= 0x80 {
            (length_byte - 0x80) as usize
        } else {
            length_byte as usize
        };

        // Special case for command 0x87
        if self.buffer[2] == 0x87 {
            self.expected_length = 3;
        }

        true
    }

    fn validate_message(&self) -> bool {
        if self.buffer.len() < MSG_HEADER_SIZE {
            return false;
        }
        let total_length = MSG_HEADER_SIZE + self.expected_length;
        if self.buffer.len() < total_length {
            return false;
        }
        if self.buffer.len() > MSG_MAX_SIZE {
            return false;
        }
        novus_validate_checksum(&self.buffer)
    }

    fn skip_potential_message(&mut self) {
        if !self.buffer.is_empty() {
            self.buffer.remove(0);
        }
        self.state = BusParserState::WaitingHeader;
    }

    fn handle_invalid_message(&mut self) {
        debug!("Bus parser: invalid message, skipping");
        self.skip_potential_message();
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.state = BusParserState::WaitingHeader;
        self.expected_length = 0;
        self.message_start_time = Instant::now();
    }
}

/// Serial port handler for Paul Novus bus communication.
pub struct SerialHandler {
    port: String,
    baudrate: u32,
    serial: Option<SerialStream>,
    parser: BusMessageParser,
    consecutive_errors: u32,
    max_consecutive_errors: u32,
    pub fatal_error: Option<String>,
    pub shutdown_requested: bool,
    pub needs_recovery: bool,
}

impl SerialHandler {
    pub fn new(port: String, baudrate: u32) -> Self {
        Self {
            port,
            baudrate,
            serial: None,
            parser: BusMessageParser::new(),
            consecutive_errors: 0,
            max_consecutive_errors: 10,
            fatal_error: None,
            shutdown_requested: false,
            needs_recovery: false,
        }
    }

    /// Open the serial port connection.
    pub fn connect(&mut self) -> Result<()> {
        let serial = tokio_serial::new(&self.port, self.baudrate)
            .stop_bits(tokio_serial::StopBits::One)
            .parity(tokio_serial::Parity::None) // overridden below with Mark parity via termios
            .data_bits(tokio_serial::DataBits::Eight)
            .timeout(std::time::Duration::from_millis(100))
            .open_native_async()?;

        // The Paul Novus bus requires Mark parity (parity bit always 1).
        // The serialport crate only supports None/Odd/Even, so we set it
        // via raw termios using the Linux-specific CMSPAR flag.
        Self::set_mark_parity(&serial)?;

        info!(port = %self.port, "Serial: connected with Mark parity");
        self.serial = Some(serial);
        self.fatal_error = None;
        Ok(())
    }

    /// Set Mark parity on a serial port via raw termios.
    ///
    /// Mark parity (CMSPAR + PARODD + PARENB) forces the parity bit to 1
    /// on every transmitted byte. This is required by the Novus bus protocol.
    fn set_mark_parity(serial: &SerialStream) -> Result<()> {
        use std::os::unix::io::AsRawFd;

        let fd = serial.as_raw_fd();
        // SAFETY: fd is valid (borrowed from an open SerialStream); termios is initialized
        // by tcgetattr before use; CMSPAR/PARODD/PARENB are valid c_cflag bits on Linux.
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut termios) != 0 {
                return Err(anyhow::anyhow!(
                    "Serial: tcgetattr failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // PARENB: enable parity, PARODD: odd parity, CMSPAR: stick parity
            // Together CMSPAR+PARODD+PARENB = Mark parity (bit always 1)
            termios.c_cflag |= libc::PARENB | libc::PARODD | libc::CMSPAR;
            if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
                return Err(anyhow::anyhow!(
                    "Serial: tcsetattr failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        info!("Serial: Mark parity set via termios (CMSPAR+PARODD+PARENB)");
        Ok(())
    }

    /// Close the serial port.
    pub fn disconnect(&mut self) {
        if let Some(serial) = self.serial.take() {
            drop(serial);
            info!("Serial: disconnected");
        }
    }

    /// Read and parse messages from the serial port.
    ///
    /// Returns a list of complete, validated messages.
    pub async fn read_messages(&mut self) -> Vec<Vec<u8>> {
        let serial = match self.serial.as_mut() {
            Some(s) => s,
            None => return Vec::new(),
        };

        let mut buf = [0u8; 128];
        // Bounded wait: a silent bus (unit off, adapter unplugged) must not
        // park the main loop, which also drains MQTT commands and feeds the
        // systemd watchdog.
        let read = match tokio::time::timeout(READ_TIMEOUT, serial.read(&mut buf)).await {
            Ok(result) => result,
            Err(_) => return Vec::new(),
        };
        match read {
            Ok(0) => Vec::new(),
            Ok(n) => {
                let data = &buf[..n];
                let messages = self.parser.add_data(data);

                if !messages.is_empty() {
                    debug!(
                        bytes = n,
                        hex = %hex_str(data),
                        count = messages.len(),
                        "Serial: parsed complete message(s)"
                    );
                }

                self.consecutive_errors = 0;
                messages
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Vec::new(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Vec::new(),
            Err(e) => {
                self.consecutive_errors += 1;
                error!(
                    attempt = self.consecutive_errors,
                    error = %e,
                    "Serial: read error"
                );

                if self.consecutive_errors >= self.max_consecutive_errors {
                    error!("Serial: too many consecutive errors, attempting recovery");
                    self.consecutive_errors = 0;
                    self.needs_recovery = true;
                }
                Vec::new()
            }
        }
    }

    /// Write raw bytes to the serial port. Bounded by `WRITE_TIMEOUT` so a stuck
    /// adapter (e.g. USB hub disconnect) can't hang the main loop indefinitely.
    pub async fn write_data(&mut self, data: &[u8]) -> bool {
        const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
        let serial = match self.serial.as_mut() {
            Some(s) => s,
            None => return false,
        };

        match tokio::time::timeout(WRITE_TIMEOUT, serial.write_all(data)).await {
            Ok(Ok(())) => true,
            Ok(Err(e)) => {
                error!(error = %e, "Serial: write error");
                false
            }
            Err(_) => {
                error!(
                    timeout_secs = WRITE_TIMEOUT.as_secs(),
                    bytes = data.len(),
                    "Serial: write timed out"
                );
                false
            }
        }
    }

    /// Async recovery with retries and escalating backoff. Call from the main loop
    /// when `needs_recovery` is set.
    ///
    /// Retry schedule: 10s, 10s, 10s, 15s, 15s, 15s (~75s total)
    /// This gives USB re-enumeration and temporary disconnects time to resolve.
    pub async fn recover(&mut self) {
        self.needs_recovery = false;
        let delays = [10u64, 10, 10, 15, 15, 15];

        for (i, &delay) in delays.iter().enumerate() {
            let attempt = i + 1;
            let level = if attempt <= 2 { "info" } else { "warn" };
            match level {
                "info" => info!(
                    attempt,
                    delay_secs = delay,
                    total = delays.len(),
                    "Serial: recovery attempt"
                ),
                _ => warn!(
                    attempt,
                    delay_secs = delay,
                    total = delays.len(),
                    "Serial: recovery attempt"
                ),
            }
            self.disconnect();
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            self.parser = BusMessageParser::new();
            match self.connect() {
                Ok(()) => {
                    info!(attempt, "Serial: recovery successful");
                    self.consecutive_errors = 0;
                    return;
                }
                Err(e) => {
                    error!(error = %e, attempt, "Serial: recovery attempt failed");
                }
            }
        }
        error!(
            "Serial: all {} recovery attempts failed after ~{}s, entering fatal state",
            delays.len(),
            delays.iter().sum::<u64>()
        );
        self.fatal_error = Some("serial_recovery_failed".into());
        self.shutdown_requested = true;
    }

    /// Build a compact human-readable summary for a parsed bus message.
    #[allow(dead_code)]
    pub fn summarize_message(message: &[u8]) -> String {
        if message.len() < MSG_HEADER_SIZE {
            return "invalid".to_string();
        }

        let cmd = message[2];
        let device = message[1];
        let payload = &message[6..];
        let cmd_str = command_name(cmd);

        let payload_hex: String = payload.iter().take(8).map(|b| format!("{b:02x}")).collect();
        let ellipsis = if payload.len() > 8 { "..." } else { "" };

        format!(
            "cmd=0x{cmd:02x}({cmd_str}),dev=0x{device:02x},\
             payload_len={},payload={payload_hex}{ellipsis}",
            payload.len()
        )
    }
}

/// Format bytes as a hex string for logging.
fn hex_str(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::novus_checksum;

    /// Build a valid test message with correct checksum.
    fn make_valid_message(device: u8, cmd: u8, data: &[u8]) -> Vec<u8> {
        let first_byte = 0x01u8;
        let length = data.len() as u8;

        // Header without checksum
        let header = vec![first_byte, device, cmd, length];
        let cs = novus_checksum(&[&header[..], data].concat());

        let mut msg = header;
        msg.push(cs[0]);
        msg.push(cs[1]);
        msg.extend_from_slice(data);
        msg
    }

    #[test]
    fn test_parser_valid_message() {
        let mut parser = BusMessageParser::new();
        let msg = make_valid_message(0x01, 0x85, &[0x10, 0x20]);
        let results = parser.add_data(&msg);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], msg);
    }

    #[test]
    fn test_parser_invalid_first_byte() {
        let mut parser = BusMessageParser::new();
        let results = parser.add_data(&[0xFF, 0x01, 0x85, 0x02, 0x00, 0x00, 0x10, 0x20]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parser_buffer_overflow() {
        let mut parser = BusMessageParser::new();
        let big = vec![0x01; MSG_MAX_SIZE + 10];
        let results = parser.add_data(&big);
        assert!(results.is_empty());
    }

    #[test]
    fn test_summarize_message() {
        let msg = make_valid_message(0x01, 0x85, &[0xAB, 0xCD]);
        let summary = SerialHandler::summarize_message(&msg);
        assert!(summary.contains("mem_write"));
        assert!(summary.contains("dev=0x01"));
    }

    #[test]
    fn test_parser_multiple_messages_in_stream() {
        let mut parser = BusMessageParser::new();
        let msg1 = make_valid_message(0x01, 0x85, &[0x10, 0x20]);
        let msg2 = make_valid_message(0x02, 0x84, &[]);
        let mut stream = Vec::new();
        stream.extend_from_slice(&msg1);
        stream.extend_from_slice(&msg2);
        let results = parser.add_data(&stream);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_parser_fragmented_delivery() {
        // Message arrives in 2 chunks (simulates serial read fragmentation)
        let mut parser = BusMessageParser::new();
        let msg = make_valid_message(0x01, 0x85, &[0x10, 0x20, 0x30]);
        let mid = msg.len() / 2;
        let results1 = parser.add_data(&msg[..mid]);
        assert!(results1.is_empty()); // not complete yet
        let results2 = parser.add_data(&msg[mid..]);
        assert_eq!(results2.len(), 1);
        assert_eq!(results2[0], msg);
    }

    #[test]
    fn test_parser_garbage_before_valid_message() {
        let mut parser = BusMessageParser::new();
        let msg = make_valid_message(0x01, 0x85, &[0xAA]);
        let mut stream = vec![0xFF, 0xFE, 0xFD]; // garbage
        stream.extend_from_slice(&msg);
        let results = parser.add_data(&stream);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], msg);
    }

    #[test]
    fn test_parser_invalid_command_byte() {
        let mut parser = BusMessageParser::new();
        // Valid address but invalid command (0xAA is not in BUS_COMMANDS)
        let results = parser.add_data(&[0x01, 0x01, 0xAA, 0x00, 0x00, 0x00]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parser_resets_between_messages() {
        let mut parser = BusMessageParser::new();
        // First: garbage
        parser.add_data(&[0xFF; 50]);
        // Then: valid message
        let msg = make_valid_message(0x01, 0x85, &[0x42]);
        let results = parser.add_data(&msg);
        assert_eq!(results.len(), 1);
    }
}
