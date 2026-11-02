//! Interactive console UI: keyboard listener thread, key-command dispatch,
//! activity-line emission (with pause / blackout modes), marker writes, and
//! the small termios + isatty helpers underpinning all of the above.
//!
//! Most of this only does anything when stderr is a TTY — pipe / file output
//! is left clean.

use std::io::{IsTerminal, Read as _};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tracing::info;

use crate::protocol;

use super::diagnostics::read_cpu_temperature;
use super::write_queue::WriteItem;
use super::{DisplayMode, KeyCommand, NovusConnector, PAUSE_BACKLOG_CAP};

impl NovusConnector {
    pub(super) fn spawn_key_listener(&mut self) {
        let (tx, rx) = mpsc::channel::<KeyCommand>(16);
        self.key_rx = Some(rx);

        // Install panic hook to restore terminal if we crash
        let original_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // SAFETY: Best-effort terminal restoration on panic. fd 0 (stdin) is valid;
            // tcgetattr/tcsetattr are standard POSIX. Failure is silently ignored.
            unsafe {
                let mut term: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(0, &mut term) == 0 {
                    term.c_lflag |= libc::ECHO | libc::ICANON;
                    libc::tcsetattr(0, libc::TCSANOW, &term);
                }
            }
            original_hook(info);
        }));

        let shutdown = self.shutdown.clone();
        let muted = self.activity_muted.clone();
        std::thread::spawn(move || {
            if !atty_is_tty() {
                return;
            }

            // Set terminal to raw mode (single keypress, no echo)
            let original_termios = match enable_raw_mode() {
                Some(t) => t,
                None => return,
            };

            let stdin = std::io::stdin();
            let mut handle = stdin.lock();
            let mut buf = [0u8; 1];

            // Line-entry state for the highlight prompt. When line_mode is true,
            // each byte is accumulated into line_buf with manual echo until Enter.
            let mut line_mode = false;
            let mut line_buf = String::new();

            loop {
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }

                match handle.read(&mut buf) {
                    Ok(1) => {
                        let byte = buf[0];

                        // Inside the highlight prompt: we're on the ALT SCREEN
                        // buffer with activity muted. Accumulate typed chars
                        // until Enter / ESC / Ctrl+C; then exit alt screen,
                        // unmute, and send the collected input.
                        if line_mode {
                            use std::io::Write;
                            let mut err = std::io::stderr();
                            let finish = |err: &mut std::io::Stderr, muted: &Arc<std::sync::atomic::AtomicBool>| {
                                // Leave alt screen; restore the main view.
                                let _ = err.write_all(b"\x1b[?1049l");
                                let _ = err.flush();
                                muted.store(false, Ordering::SeqCst);
                            };
                            match byte {
                                b'\r' | b'\n' => {
                                    line_mode = false;
                                    finish(&mut err, &muted);
                                    let text = std::mem::take(&mut line_buf);
                                    if tx.blocking_send(KeyCommand::HighlightInput(text)).is_err() {
                                        break;
                                    }
                                }
                                0x1b | 3 => {
                                    // ESC or Ctrl+C: cancel
                                    line_mode = false;
                                    line_buf.clear();
                                    finish(&mut err, &muted);
                                }
                                0x7f | 0x08 => {
                                    // Backspace / DEL
                                    if line_buf.pop().is_some() {
                                        let _ = err.write_all(b"\x08 \x08");
                                        let _ = err.flush();
                                    }
                                }
                                b if (b.is_ascii_graphic() || b == b' ') && line_buf.len() < 16 => {
                                    line_buf.push(b as char);
                                    let _ = err.write_all(&[b]);
                                    let _ = err.flush();
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // Normal key dispatch.
                        let mut to_send: Vec<KeyCommand> = Vec::new();
                        match byte {
                            b'c' | b'C' => to_send.push(KeyCommand::Clear),
                            b'd' => to_send.push(KeyCommand::Dump),
                            b'D' => {
                                to_send.push(KeyCommand::Clear);
                                to_send.push(KeyCommand::Dump);
                            }
                            b't' => to_send.push(KeyCommand::TimeSync),
                            b'T' => {
                                to_send.push(KeyCommand::Clear);
                                to_send.push(KeyCommand::TimeSync);
                            }
                            b's' => to_send.push(KeyCommand::Stats),
                            b'S' => {
                                to_send.push(KeyCommand::Clear);
                                to_send.push(KeyCommand::Stats);
                            }
                            b'm' => to_send.push(KeyCommand::Marker),
                            b'M' => {
                                to_send.push(KeyCommand::Clear);
                                to_send.push(KeyCommand::Marker);
                            }
                            b'f' => to_send.push(KeyCommand::Bookmark),
                            b'F' => {
                                to_send.push(KeyCommand::Clear);
                                to_send.push(KeyCommand::Bookmark);
                            }
                            b'p' => to_send.push(KeyCommand::EnterPause),
                            b'P' => {
                                to_send.push(KeyCommand::Clear);
                                to_send.push(KeyCommand::EnterPause);
                            }
                            b'b' => to_send.push(KeyCommand::EnterBlackout),
                            b'B' => {
                                to_send.push(KeyCommand::Clear);
                                to_send.push(KeyCommand::EnterBlackout);
                            }
                            b'r' => to_send.push(KeyCommand::Resume),
                            b'R' => {
                                to_send.push(KeyCommand::Clear);
                                to_send.push(KeyCommand::Resume);
                            }
                            b'h' => {
                                // Switch to alt-screen buffer and mute activity
                                // stream so the prompt is never overwritten by
                                // a new RECV/SEND line.
                                use std::io::Write;
                                let mut err = std::io::stderr();
                                muted.store(true, Ordering::SeqCst);
                                // Enter alt screen, clear it, move cursor home.
                                let _ = err.write_all(b"\x1b[?1049h\x1b[2J\x1b[H");
                                let _ = writeln!(err, "┏━━ highlight a register ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                                let _ = writeln!(err, "┃");
                                let _ = writeln!(
                                    err,
                                    "┃  Type a register address (e.g. 0x55, or just 55) + Enter"
                                );
                                let _ =
                                    writeln!(err, "┃  Leave blank or press ESC / Ctrl+C to cancel");
                                let _ = writeln!(err, "┃");
                                let _ = writeln!(err, "┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                                let _ = writeln!(err);
                                let _ = err.write_all(b"\xe2\x86\xaa "); // "↪ "
                                let _ = err.flush();
                                line_mode = true;
                                line_buf.clear();
                            }
                            b'H' => to_send.push(KeyCommand::HighlightClear),
                            b'?' => to_send.push(KeyCommand::Help),
                            b'q' | b'Q' | b'x' | b'X' => {
                                shutdown.store(true, Ordering::SeqCst);
                            }
                            3 => {
                                // Ctrl+C
                                shutdown.store(true, Ordering::SeqCst);
                            }
                            _ => {}
                        }
                        for c in to_send {
                            if tx.blocking_send(c).is_err() {
                                break;
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }

            // Restore terminal on exit
            restore_terminal(original_termios);
        });
    }

    /// Route a formatted activity-log message based on the current display mode.
    ///
    /// Activity lines are written DIRECTLY to stderr with `eprintln!` rather
    /// than through `tracing::info!`, because the tracing formatter escapes
    /// control bytes (our ANSI highlight codes become literal `\x1b[...]`).
    /// Direct stderr keeps the codes as real bytes so terminals render them.
    ///
    /// - `Live`     → immediate eprintln with `[t+ Δ] reg: <body>` prefix
    /// - `Paused`   → buffer the full formatted line; flushed on Resume
    /// - `Blackout` → drop and bump the counter
    ///
    /// If `address` is in the highlight set AND stderr is a TTY, the body is
    /// wrapped in ANSI bold-bright-white so it stands out.
    pub(super) fn emit_activity_line(&mut self, address: &str, body: &str) {
        // Silently drop while the listener is showing a full-screen prompt —
        // the alt-screen view shouldn't be stomped by stray RECV/SEND lines.
        if self.activity_muted.load(Ordering::SeqCst) {
            return;
        }
        let tty = std::io::stderr().is_terminal();
        let highlighted = self.highlight_addresses.contains(address) && tty;
        let rendered = if highlighted {
            format!("\x1b[1;97m{body}\x1b[0m")
        } else {
            body.to_string()
        };

        // Build the full prefix to match tracing's human formatter:
        //   <dim>TS</dim> [t+ Δ]  <green>INFO</green> <dim>reg</dim>: body
        let wall = humantime::format_rfc3339_micros(std::time::SystemTime::now()).to_string();
        let (total, delta) = crate::logging::peek_elapsed();
        let hms = |t: u64| format!("{:02}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60);
        let delta_str = match delta {
            Some(d) => hms(d),
            None => "--:--:--".to_string(),
        };
        let (dim, green, reset) = if tty {
            ("\x1b[2m", "\x1b[32m", "\x1b[0m")
        } else {
            ("", "", "")
        };
        let line = format!(
            "{dim}{wall} [t+{} Δ{}]{reset}  {green}INFO{reset} {dim}reg{reset}: {rendered}",
            hms(total),
            delta_str,
        );

        match self.display_mode {
            DisplayMode::Live => {
                eprintln!("{line}");
            }
            DisplayMode::Paused => {
                let dur = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                let secs = dur.as_secs();
                let ms = dur.subsec_millis();
                let h = (secs / 3600) % 24;
                let m = (secs / 60) % 60;
                let s = secs % 60;
                let buffered = format!("  (buffered {h:02}:{m:02}:{s:02}.{ms:03}Z) {line}");
                if self.pause_backlog.len() >= PAUSE_BACKLOG_CAP {
                    self.pause_backlog.pop_front();
                    self.pause_backlog_overflow = self.pause_backlog_overflow.saturating_add(1);
                }
                self.pause_backlog.push_back(buffered);
            }
            DisplayMode::Blackout => {
                self.blackout_dropped = self.blackout_dropped.saturating_add(1);
            }
        }
    }

    pub(super) fn write_marker_line(&mut self, kind: &str, tag: &str) {
        let (since_start, since_last) = crate::logging::stamp_marker();
        let fmt = |s: u64| format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60);
        let t_elapsed = fmt(since_start);
        let t_delta = match since_last {
            Some(s) => fmt(s),
            None => "--:--:--".to_string(),
        };

        // ANSI color — bold yellow for MARKER, bold red for BOOKMARK.
        // Only emit escapes if stderr is a TTY (pipes/files stay clean).
        let (c_on, c_off) = if std::io::stderr().is_terminal() {
            let code = match kind {
                "MARKER" => "\x1b[1;33m",
                "BOOKMARK" => "\x1b[1;31m",
                _ => "",
            };
            (code, if code.is_empty() { "" } else { "\x1b[0m" })
        } else {
            ("", "")
        };

        let suffix = if self.bus_dump.is_some() {
            "→ written to bus dump"
        } else {
            "(bus dump not active)"
        };
        eprintln!("{c_on}⚑ {kind} {tag}  [t+{t_elapsed}]  [Δ{t_delta}]  {suffix}{c_off}");

        if let Some(ref mut dump) = self.bus_dump {
            use std::io::Write;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            let _ = writeln!(dump, "{ts:.3} # {kind} {tag} t+{t_elapsed} Δ{t_delta}");
            let _ = dump.flush();
        }
        info!(kind = %kind, tag = %tag, t_elapsed = %t_elapsed, t_delta = %t_delta, "Keyboard: marker inserted");
    }

    pub(super) async fn process_key_commands(&mut self) {
        if let Some(mut rx) = self.key_rx.take() {
            let mut cmds = Vec::new();
            while let Ok(cmd) = rx.try_recv() {
                cmds.push(cmd);
            }
            self.key_rx = Some(rx);

            for cmd in cmds {
                match cmd {
                    KeyCommand::Dump => {
                        info!("Keyboard: memory dump requested");
                        let dump_msg = protocol::build_bus_message(&[0x01, 0x01], 0x83, &[]);
                        self.bus_write_stack.push_back(WriteItem::Raw(dump_msg));
                    }
                    KeyCommand::TimeSync => {
                        info!("Keyboard: forcing time sync");
                        self.publish_time_sync().await;
                    }
                    KeyCommand::Stats => {
                        let snapshot = self.metrics.snapshot();
                        let serial_ok = self
                            .serial_handler
                            .as_ref()
                            .map(|s| s.fatal_error.is_none())
                            .unwrap_or(false);
                        let uptime = snapshot.uptime_seconds as u64;
                        eprintln!("─── novus2mqtt stats ───");
                        eprintln!(
                            "  Uptime:        {:03}d {:02}:{:02}:{:02}",
                            uptime / 86400,
                            (uptime % 86400) / 3600,
                            (uptime % 3600) / 60,
                            uptime % 60
                        );
                        eprintln!(
                            "  Bus:           {} (rx:{} tx:{} err:{})",
                            if serial_ok { "ok" } else { "ERROR" },
                            snapshot.bus_messages_received,
                            snapshot.bus_messages_sent,
                            snapshot.errors_count
                        );
                        eprintln!(
                            "  Registers:     {}/{}",
                            self.registers_seen.len(),
                            self.address_metadata.len()
                        );
                        eprintln!(
                            "  MQTT:          pub:{} recv:{}",
                            snapshot.mqtt_messages_sent, snapshot.mqtt_messages_received
                        );
                        eprintln!("  Memory:        {:.1} MB", snapshot.memory_usage_mb);
                        eprintln!(
                            "  CPU:           {:.1}%  temp:{:.1}°C",
                            snapshot.cpu_percent,
                            read_cpu_temperature()
                        );
                        eprintln!("  Write queue:   {} pending", self.bus_write_stack.len());
                        eprintln!("────────────────────────");
                    }
                    KeyCommand::Help => {
                        eprintln!("─── novus2mqtt commands ───");
                        eprintln!(
                            "  d / D       Request memory dump  (uppercase = clear screen first)"
                        );
                        eprintln!("  t / T       Force time sync");
                        eprintln!("  s / S       Show live statistics");
                        eprintln!(
                            "  m / M       Marker (M1,M2,...)   (uppercase = clear screen first)"
                        );
                        eprintln!(
                            "  f / F       Bookmark (B1,B2,...) (uppercase = clear screen first)"
                        );
                        eprintln!("  p / P       Pause activity stream — new lines are BUFFERED; 'r' replays them");
                        eprintln!("  b / B       Blackout activity stream — new lines are DROPPED; 'r' resumes live");
                        eprintln!(
                            "  r / R       Resume live activity stream (replays backlog if paused)"
                        );
                        eprintln!(
                            "  h           Highlight register — prompts for address (e.g. 0x55)"
                        );
                        eprintln!("  H           Clear all highlights");
                        eprintln!("  c / C       Clear screen");
                        eprintln!("  ?           Show this help");
                        eprintln!("  q / x       Shutdown gracefully");
                        if !self.highlight_addresses.is_empty() {
                            let mut addrs: Vec<_> =
                                self.highlight_addresses.iter().cloned().collect();
                            addrs.sort();
                            eprintln!("  active highlights: {}", addrs.join(", "));
                        }
                        eprintln!("───────────────────────────");
                    }
                    KeyCommand::HighlightInput(raw) => {
                        match parse_register_address(&raw) {
                            Some(addr) => {
                                let newly = self.highlight_addresses.insert(addr.clone());
                                let count = self.highlight_addresses.len();
                                if newly {
                                    eprintln!("✨ highlighting {addr} ({count} total)");
                                } else {
                                    eprintln!("✨ {addr} already highlighted ({count} total)");
                                }
                            }
                            None if raw.trim().is_empty() => {
                                // Empty input — user just cancelled. Silent.
                            }
                            None => {
                                eprintln!(
                                    "✗ not a valid register address: {:?} (expected e.g. 0x55)",
                                    raw
                                );
                            }
                        }
                    }
                    KeyCommand::HighlightClear => {
                        let n = self.highlight_addresses.len();
                        self.highlight_addresses.clear();
                        if n > 0 {
                            eprintln!("✨ cleared {n} highlight{}", if n == 1 { "" } else { "s" });
                        } else {
                            eprintln!("✨ no active highlights");
                        }
                    }
                    KeyCommand::EnterPause => match self.display_mode {
                        DisplayMode::Live => {
                            self.display_mode = DisplayMode::Paused;
                            self.mode_entered_at = Some(Instant::now());
                            self.pause_backlog.clear();
                            self.pause_backlog_overflow = 0;
                            eprintln!(
                                "⏸  Paused — activity lines will be BUFFERED (cap {PAUSE_BACKLOG_CAP}); press 'r' to resume and replay"
                            );
                            eprintln!(
                                "    (bus processing, MQTT publish, bus dump and throttling all continue normally)"
                            );
                        }
                        DisplayMode::Paused => {
                            eprintln!("⏸  Already paused — press 'r' to resume");
                        }
                        DisplayMode::Blackout => {
                            let dropped = self.blackout_dropped;
                            self.display_mode = DisplayMode::Paused;
                            self.mode_entered_at = Some(Instant::now());
                            self.pause_backlog.clear();
                            self.pause_backlog_overflow = 0;
                            self.blackout_dropped = 0;
                            eprintln!(
                                "⏸  Switched blackout → paused; dropped {dropped} lines in blackout, buffering starts now"
                            );
                        }
                    },
                    KeyCommand::EnterBlackout => match self.display_mode {
                        DisplayMode::Live => {
                            self.display_mode = DisplayMode::Blackout;
                            self.mode_entered_at = Some(Instant::now());
                            self.blackout_dropped = 0;
                            eprintln!("⬛  Blackout — activity lines will be DROPPED; press 'r' to resume");
                            eprintln!(
                                "    (bus processing, MQTT publish, bus dump and throttling all continue normally)"
                            );
                        }
                        DisplayMode::Blackout => {
                            eprintln!("⬛  Already in blackout — press 'r' to resume");
                        }
                        DisplayMode::Paused => {
                            let dropped =
                                self.pause_backlog.len() as u32 + self.pause_backlog_overflow;
                            self.pause_backlog.clear();
                            self.pause_backlog_overflow = 0;
                            self.display_mode = DisplayMode::Blackout;
                            self.mode_entered_at = Some(Instant::now());
                            self.blackout_dropped = 0;
                            eprintln!(
                                "⬛  Switched pause → blackout; discarded {dropped} buffered lines"
                            );
                        }
                    },
                    KeyCommand::Resume => match self.display_mode {
                        DisplayMode::Live => {
                            eprintln!("▶  Already live");
                        }
                        DisplayMode::Paused => {
                            let elapsed = self
                                .mode_entered_at
                                .map(|t| t.elapsed().as_secs())
                                .unwrap_or(0);
                            let h = elapsed / 3600;
                            let m = (elapsed / 60) % 60;
                            let s = elapsed % 60;
                            let count = self.pause_backlog.len();
                            let overflow = self.pause_backlog_overflow;
                            let overflow_note = if overflow > 0 {
                                format!(" (+{overflow} dropped at {PAUSE_BACKLOG_CAP} cap)")
                            } else {
                                String::new()
                            };
                            eprintln!(
                                "▶  Resumed from pause ({h:02}:{m:02}:{s:02}) — replaying {count} buffered lines{overflow_note}"
                            );
                            for line in self.pause_backlog.drain(..) {
                                eprintln!("{line}");
                            }
                            eprintln!("▶  End of replay — live");
                            self.pause_backlog_overflow = 0;
                            self.display_mode = DisplayMode::Live;
                            self.mode_entered_at = None;
                        }
                        DisplayMode::Blackout => {
                            let elapsed = self
                                .mode_entered_at
                                .map(|t| t.elapsed().as_secs())
                                .unwrap_or(0);
                            let h = elapsed / 3600;
                            let m = (elapsed / 60) % 60;
                            let s = elapsed % 60;
                            let dropped = self.blackout_dropped;
                            eprintln!(
                                "▶  Resumed from blackout ({h:02}:{m:02}:{s:02}) — {dropped} lines dropped"
                            );
                            self.blackout_dropped = 0;
                            self.display_mode = DisplayMode::Live;
                            self.mode_entered_at = None;
                        }
                    },
                    KeyCommand::Clear => {
                        use std::io::Write;
                        if std::io::stderr().is_terminal() {
                            // ANSI: clear screen + move cursor home.
                            eprint!("\x1b[2J\x1b[H");
                            let _ = std::io::stderr().flush();
                        }
                    }
                    KeyCommand::Marker => {
                        self.marker_counter += 1;
                        let tag = format!("M{}", self.marker_counter);
                        self.write_marker_line("MARKER", &tag);
                    }
                    KeyCommand::Bookmark => {
                        self.bookmark_counter += 1;
                        let tag = format!("B{}", self.bookmark_counter);
                        self.write_marker_line("BOOKMARK", &tag);
                    }
                }
            }
        }
    }
}

/// Parse user-typed register address input. Accepts forms like `0x55`, `0X55`,
/// `55`, `5`. Returns the canonical lower-case `0xNN` form, or None if the
/// input isn't a valid 1-byte hex address.
fn parse_register_address(input: &str) -> Option<String> {
    let trimmed = input.trim().to_lowercase();
    let hex = trimmed.strip_prefix("0x").unwrap_or(&trimmed);
    if hex.is_empty() || hex.len() > 2 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = u8::from_str_radix(hex, 16).ok()?;
    Some(format!("0x{n:02x}"))
}

/// Check if stdin is a terminal (not piped/redirected).
fn atty_is_tty() -> bool {
    // SAFETY: isatty is a standard POSIX function; fd 0 (stdin) is always valid.
    unsafe { libc::isatty(0) != 0 }
}

/// Set terminal to raw mode (no echo, no line buffering). Returns original settings.
fn enable_raw_mode() -> Option<libc::termios> {
    // SAFETY: fd 0 (stdin) is valid; termios is populated by tcgetattr before modification.
    unsafe {
        let mut original: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut original) != 0 {
            return None;
        }
        let mut raw = original;
        raw.c_lflag &= !(libc::ECHO | libc::ICANON);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(0, libc::TCSANOW, &raw) != 0 {
            return None;
        }
        Some(original)
    }
}

/// Restore terminal to original settings.
fn restore_terminal(original: libc::termios) {
    // SAFETY: original was obtained from a successful tcgetattr; fd 0 is valid.
    unsafe {
        libc::tcsetattr(0, libc::TCSANOW, &original);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_register_address;

    #[test]
    fn parse_address_accepts_canonical() {
        assert_eq!(parse_register_address("0x55"), Some("0x55".into()));
        assert_eq!(parse_register_address("0X55"), Some("0x55".into()));
    }

    #[test]
    fn parse_address_accepts_bare_hex() {
        assert_eq!(parse_register_address("55"), Some("0x55".into()));
        assert_eq!(parse_register_address("5"), Some("0x05".into()));
        assert_eq!(parse_register_address("ff"), Some("0xff".into()));
    }

    #[test]
    fn parse_address_normalizes_case_and_whitespace() {
        assert_eq!(parse_register_address(" 0xFF "), Some("0xff".into()));
    }

    #[test]
    fn parse_address_rejects_garbage() {
        assert_eq!(parse_register_address(""), None);
        assert_eq!(parse_register_address("xyz"), None);
        assert_eq!(parse_register_address("0x123"), None); // too long
        assert_eq!(parse_register_address("hello"), None);
    }
}
