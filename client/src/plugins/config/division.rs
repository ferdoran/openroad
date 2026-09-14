//! Gateway configuration Joymax ships inside `Media.pk2`.
//!
//! Two files at the archive root carry what the client needs before it can
//! reach a server, and both were previously ignored in favour of constants:
//!
//! * `divisioninfo.txt` — binary despite the extension: the content id, then
//!   the division list, each division naming its gateway hosts.
//! * `gateport.txt` — the gateway port as NUL-padded ASCII decimal.
//!
//! These are read synchronously through [`Archive::read_file_bytes`] rather
//! than the asset server, because the gateway connect happens on the first
//! frame (`OnEnter(SceneState::Loading)`) and the headless net-check path never
//! builds an asset server at all. Both are optional: a missing or malformed
//! file falls back to [`DivisionInfo::FALLBACK_CONTENT_ID`] and leaves the
//! gateway to `config.yaml`, so a tree without PK2s keeps working.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use bevy::prelude::{warn, Resource};
use bevy_pk2::prelude::{Archive, Pk2Key};

/// One division and the gateway hosts that serve it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Division {
    pub name: String,
    pub gateways: Vec<String>,
}

/// Parsed `divisioninfo.txt` + `gateport.txt`.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct DivisionInfo {
    /// `content_id` for the gateway/agent login packets.
    pub content_id: u8,
    pub divisions: Vec<Division>,
    /// From `gateport.txt`; `None` when that file is absent or unparsable.
    pub gateway_port: Option<u16>,
}

impl Default for DivisionInfo {
    fn default() -> Self {
        Self {
            content_id: Self::FALLBACK_CONTENT_ID,
            divisions: Vec::new(),
            gateway_port: None,
        }
    }
}

impl DivisionInfo {
    /// The literal the five login call sites carried before this file was
    /// read. Kept as the fallback so a tree without `Media.pk2` — notably the
    /// headless net-check against a local stub — behaves exactly as before.
    pub const FALLBACK_CONTENT_ID: u8 = 22;

    /// Reads both files from `Media.pk2`, or returns the fallback with a
    /// warning. Resolves the archive the same way `SroAssetPlugin` does:
    /// `SRO_PK2_PATH` → `SRO_PATH` → `<cwd>/assets`.
    pub fn load_or_fallback() -> Self {
        match Self::load_from_pk2() {
            Some(info) => info,
            None => {
                warn!(
                    "could not read divisioninfo.txt/gateport.txt from Media.pk2 — \
                     falling back to content id {} and the configured gateway",
                    Self::FALLBACK_CONTENT_ID
                );
                Self::default()
            }
        }
    }

    fn load_from_pk2() -> Option<Self> {
        let media = media_pk2_path();
        // `open_or_panic` panics rather than returning an error, and this runs
        // before the asset plugin would have reported a missing archive.
        // A missing key is the same class of "no archive to read" as a missing
        // file here, so it takes the same silent fallback rather than aborting
        // startup — the asset plugin reports it properly a moment later.
        let key = Pk2Key::resolve().ok()?;
        panic::catch_unwind(AssertUnwindSafe(|| {
            let archive = Archive::open_or_panic(&media, &key);
            let mut info = Self::parse(&archive.read_file_bytes(Path::new("divisioninfo.txt"))?)?;
            info.gateway_port = archive
                .read_file_bytes(Path::new("gateport.txt"))
                .as_deref()
                .and_then(parse_gateport);
            Some(info)
        }))
        .ok()
        .flatten()
    }

    /// Parses `divisioninfo.txt`:
    /// `u8 content_id | u8 division_count | { u32 len, name, 0x00, u8 gateway_count, { u32 len, host, 0x00 } }`.
    ///
    /// Every string is a `u32` length (excluding the terminator) followed by
    /// its bytes and a `0x00` byte — the wiki's `//'0'` separator comment is a
    /// typo for byte `0x00`.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let mut at = 0usize;
        let content_id = *bytes.get(at)?;
        at += 1;
        let division_count = *bytes.get(at)?;
        at += 1;

        let mut divisions = Vec::with_capacity(division_count as usize);
        for _ in 0..division_count {
            let name = read_string(bytes, &mut at)?;
            let gateway_count = *bytes.get(at)?;
            at += 1;
            let mut gateways = Vec::with_capacity(gateway_count as usize);
            for _ in 0..gateway_count {
                gateways.push(read_string(bytes, &mut at)?);
            }
            divisions.push(Division { name, gateways });
        }

        Some(Self {
            content_id,
            divisions,
            gateway_port: None,
        })
    }

    /// `host:port` of the first gateway of the first division, when both the
    /// host and `gateport.txt` are available.
    pub fn gateway_address(&self) -> Option<String> {
        let host = self.divisions.first()?.gateways.first()?;
        Some(format!("{host}:{}", self.gateway_port?))
    }
}

/// `u32` length + bytes + a `0x00` terminator.
fn read_string(bytes: &[u8], at: &mut usize) -> Option<String> {
    let len = u32::from_le_bytes(bytes.get(*at..*at + 4)?.try_into().ok()?) as usize;
    *at += 4;
    let raw = bytes.get(*at..*at + len)?;
    *at += len;
    // The terminator must be present and zero, otherwise the record is not the
    // shape we think it is and every later offset would be wrong.
    if *bytes.get(*at)? != 0 {
        return None;
    }
    *at += 1;
    Some(String::from_utf8_lossy(raw).into_owned())
}

/// `gateport.txt` is ASCII decimal padded with NULs.
fn parse_gateport(bytes: &[u8]) -> Option<u16> {
    let digits: String = bytes
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .map(|b| *b as char)
        .collect();
    digits.parse().ok()
}

/// The same resolution order `SroAssetPlugin` uses, so both read one archive.
fn media_pk2_path() -> PathBuf {
    std::env::var_os("SRO_PK2_PATH")
        .or_else(|| std::env::var_os("SRO_PATH"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let mut dir = std::env::current_dir().unwrap_or_default();
            dir.push("assets");
            dir
        })
        .join("Media.pk2")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape of `Media/divisioninfo.txt` — all 36 bytes, gateway host redacted:
    /// content id 22, one division `DIV01`, one gateway `filter.example.com`.
    const REAL_DIVISIONINFO: [u8; 36] = [
        0x16, 0x01, 0x05, 0x00, 0x00, 0x00, b'D', b'I', b'V', b'0', b'1', 0x00, 0x01, 0x12, 0x00,
        0x00, 0x00, b'f', b'i', b'l', b't', b'e', b'r', b'.', b'e', b'x', b'a', b'm', b'p', b'l',
        b'e', b'.', b'c', b'o', b'm', 0x00,
    ];

    #[test]
    fn parses_the_real_divisioninfo() {
        let info = DivisionInfo::parse(&REAL_DIVISIONINFO).expect("real file must parse");
        assert_eq!(info.content_id, 22);
        assert_eq!(info.divisions.len(), 1);
        assert_eq!(info.divisions[0].name, "DIV01");
        assert_eq!(info.divisions[0].gateways, vec!["filter.example.com"]);
        // The content id the five login sites hardcoded is exactly byte 0.
        assert_eq!(info.content_id, DivisionInfo::FALLBACK_CONTENT_ID);
    }

    #[test]
    fn parses_the_real_gateport() {
        // 8 bytes: "4001" then NUL padding.
        assert_eq!(parse_gateport(b"4001\0\0\0\0"), Some(4001));
    }

    #[test]
    fn builds_the_gateway_address_from_both_files() {
        let mut info = DivisionInfo::parse(&REAL_DIVISIONINFO).expect("parses");
        assert_eq!(info.gateway_address(), None, "no port yet");
        info.gateway_port = Some(4001);
        assert_eq!(
            info.gateway_address().as_deref(),
            Some("filter.example.com:4001")
        );
    }

    #[test]
    fn rejects_truncated_and_unterminated_records() {
        for cut in 0..REAL_DIVISIONINFO.len() {
            assert!(
                DivisionInfo::parse(&REAL_DIVISIONINFO[..cut]).is_none(),
                "{cut}-byte prefix must not parse"
            );
        }
        // A non-zero byte where the terminator belongs means the layout is not
        // what we assume, so every later offset would be garbage.
        let mut unterminated = REAL_DIVISIONINFO;
        unterminated[11] = b'X';
        assert!(DivisionInfo::parse(&unterminated).is_none());
    }

    #[test]
    fn a_bogus_length_does_not_panic() {
        let mut bad = REAL_DIVISIONINFO;
        bad[2..6].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(DivisionInfo::parse(&bad).is_none());
    }

    #[test]
    fn gateport_without_digits_is_none() {
        assert_eq!(parse_gateport(b"\0\0\0\0"), None);
        assert_eq!(parse_gateport(b""), None);
    }

    #[test]
    fn fallback_keeps_the_previous_content_id() {
        let info = DivisionInfo::default();
        assert_eq!(info.content_id, 22);
        assert_eq!(info.gateway_address(), None);
    }

    /// End-to-end through the real archive, since the unit tests above only
    /// prove the parser. Run with:
    /// `SRO_PK2_PATH=/path/to/pk2 cargo test -p client reads_both_files_from_media_pk2 -- --ignored`
    #[test]
    #[ignore = "needs Media.pk2; set SRO_PK2_PATH"]
    fn reads_both_files_from_media_pk2() {
        let info = DivisionInfo::load_from_pk2().expect("Media.pk2 must yield both files");
        assert_eq!(info.content_id, 22);
        assert_eq!(info.divisions.len(), 1);
        assert_eq!(info.divisions[0].name, "DIV01");
        assert_eq!(info.divisions[0].gateways, vec!["filter.example.com"]);
        assert_eq!(info.gateway_port, Some(4001));
        assert_eq!(
            info.gateway_address().as_deref(),
            Some("filter.example.com:4001")
        );
    }
}
