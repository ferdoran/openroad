//! Per-opcode dump of every packet payload for offline re-analysis:
//! `packet_dump/<opcode>.log` gets one `<RFC3339-ms UTC> <hex payload> <E|P>`
//! line per received packet, appended across runs.
//!
//! The third column is the frame's wire `0x8000` bit — `E` = the body arrived
//! blowfish-encrypted, `P` = plaintext (#459). Without it a length read out of
//! an old log is ambiguous: an encrypted frame's payload is padded to the
//! block, so a later analyst cannot tell a real body length from a padded one
//! (12 of 58 captured opcodes are undecidable from length alone). The flag is
//! a *suffix*, so `cut -d' ' -f2` still yields the hex payload and lines
//! written before 2026-08-14 — which simply lack a third column — stay
//! parseable, with their encryption state unknown. Payloads are captured *before*
//! deserialization so unknown/unhandled opcodes are recorded too, and file
//! handles are cached per opcode so the hot receive path only pays for a
//! buffered line write.
//!
//! Sent packets go to `packet_dump/c2s/<opcode>.log` instead, captured before
//! the frame is serialized so the body is plaintext even once the outbound
//! encryption policy is on. The separate directory is what keeps the two
//! directions apart: several opcodes (`0x2001`, `0x6100`, ...) are used in
//! both, so a shared file would interleave request and response bodies.

use bevy::log::warn;
use bevy::prelude::Resource;
use chrono::SecondsFormat;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;

const DUMP_DIR: &str = "packet_dump";
/// Subdirectory of [`DUMP_DIR`] holding the client → server direction.
const SENT_SUBDIR: &str = "c2s";

#[derive(Resource)]
pub struct PacketDump {
    /// Dump root; only tests point it anywhere but [`DUMP_DIR`].
    root: PathBuf,
    /// Keyed by (subdirectory, opcode) so the two directions never share a
    /// file handle for an opcode both of them use.
    files: HashMap<(&'static str, u16), File>,
    /// Set after the first unrecoverable I/O error so we warn once instead of
    /// spamming the log every packet.
    failed: bool,
}

impl Default for PacketDump {
    fn default() -> Self {
        Self {
            root: PathBuf::from(DUMP_DIR),
            files: HashMap::new(),
            failed: false,
        }
    }
}

impl PacketDump {
    /// Record a received (server → client) payload. `encrypted` is the frame's
    /// wire `0x8000` bit, recorded as the line's third column.
    pub fn dump(&mut self, opcode: u16, data: &[u8], encrypted: bool) {
        self.write("", opcode, data, encrypted);
    }

    /// Record a sent (client → server) payload. Call before serialization so
    /// the recorded body is plaintext regardless of the encryption policy —
    /// hence the `P` column on every c2s line: it describes the bytes in the
    /// log, not what went on the wire.
    pub fn dump_sent(&mut self, opcode: u16, data: &[u8]) {
        self.write(SENT_SUBDIR, opcode, data, false);
    }

    fn write(&mut self, subdir: &'static str, opcode: u16, data: &[u8], encrypted: bool) {
        if self.failed {
            return;
        }
        if let Err(e) = self.try_dump(subdir, opcode, data, encrypted) {
            warn!("packet_dump: disabled after I/O error on {opcode:#06x}.log: {e}");
            self.failed = true;
        }
    }

    fn try_dump(
        &mut self,
        subdir: &'static str,
        opcode: u16,
        data: &[u8],
        encrypted: bool,
    ) -> std::io::Result<()> {
        if !self.files.contains_key(&(subdir, opcode)) {
            let dir = self.root.join(subdir);
            std::fs::create_dir_all(&dir)?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(format!("{opcode:#06x}.log")))?;
            self.files.insert((subdir, opcode), file);
        }
        let mut line = String::with_capacity(32 + data.len() * 2);
        line.push_str(&chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        line.push(' ');
        for byte in data {
            let _ = write!(line, "{byte:02x}");
        }
        line.push(' ');
        line.push(if encrypted { 'E' } else { 'P' });
        line.push('\n');
        self.files
            .get_mut(&(subdir, opcode))
            .expect("inserted above")
            .write_all(line.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x2001` (and the login opcodes) travel in both directions, so a shared
    /// per-opcode file would interleave request and response bodies and make
    /// the corpus useless for layout work. Sent packets must land under
    /// `c2s/`, received ones stay at the root where the existing corpus lives.
    #[test]
    fn the_two_directions_never_share_a_file() {
        let root = std::env::temp_dir().join(format!("openroad-dump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut dump = PacketDump {
            root: root.clone(),
            ..Default::default()
        };

        dump.dump(0x2001, &[0xAA], false);
        dump.dump_sent(0x2001, &[0xBB]);

        let received = std::fs::read_to_string(root.join("0x2001.log")).expect("s2c log");
        let sent = std::fs::read_to_string(root.join("c2s").join("0x2001.log")).expect("c2s log");
        assert!(received.trim_end().ends_with(" aa P"), "got {received:?}");
        assert!(sent.trim_end().ends_with(" bb P"), "got {sent:?}");

        std::fs::remove_dir_all(&root).expect("clean up");
    }

    /// #459: a dump line used to be `<ts> <hex>`, so a padded encrypted body
    /// and a plaintext one of the same length were indistinguishable after the
    /// fact. The flag is the third column — deliberately a suffix, so the
    /// documented `cut -d' ' -f2 | xxd -r -p` recipe and every pre-existing
    /// log line keep working.
    #[test]
    fn each_line_records_the_frames_encryption_bit() {
        let root = std::env::temp_dir().join(format!("openroad-dump-enc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut dump = PacketDump {
            root: root.clone(),
            ..Default::default()
        };

        dump.dump(0x3020, &[0xB5, 0xA8], true);
        dump.dump(0x3020, &[0xB5, 0xA8], false);

        let log = std::fs::read_to_string(root.join("0x3020.log")).expect("s2c log");
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines.len(), 2, "got {log:?}");
        assert!(lines[0].ends_with(" b5a8 E"), "got {:?}", lines[0]);
        assert!(lines[1].ends_with(" b5a8 P"), "got {:?}", lines[1]);
        // The payload stays field 2 for `cut -d' ' -f2`.
        for line in lines {
            assert_eq!(line.split(' ').nth(1), Some("b5a8"), "got {line:?}");
        }

        std::fs::remove_dir_all(&root).expect("clean up");
    }
}
