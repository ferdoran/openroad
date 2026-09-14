//! Shared textdata byte->String decoder (EP-09.1).
//!
//! SRO textdata string tables are UTF-16LE with a byte-order mark in the
//! v1.188 corpus; a handful of older KSRO-era tables (regioninfo.txt,
//! effectsound.txt, effectenvsnd.txt, regioncode.txt) are CP949 with no BOM.
//! The loader hand-rolled `chunks(2).skip(1) + from_utf16_lossy` inline, which
//! assumes UTF-16LE and would corrupt the CP949 tables. This module sniffs the
//! BOM and falls back to CP949 so the loader (and any future standalone
//! consumer) share one correct decode path.
//!
//! See `docs/formats/textdata-encoding.md`. Encrypted skilldata (`0xE2 0xB0`
//! header, Joymax cipher) is out of scope here and tracked in EP-09.

use encoding::all::{UTF_16BE, UTF_16LE, WINDOWS_949};
use encoding::{DecoderTrap, Encoding};

/// Decode raw textdata bytes into a `String`.
///
/// The encoding is chosen by BOM: `FF FE` -> UTF-16LE, `FE FF` -> UTF-16BE.
/// With no recognized BOM the bytes are treated as CP949 (WHATWG euc-kr, a
/// superset of EUC-KR), which also decodes plain ASCII unchanged. Invalid
/// sequences become U+FFFD rather than failing, matching the previous
/// `from_utf16_lossy` behaviour.
pub fn decode_textdata(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => UTF_16LE.decode(rest, DecoderTrap::Replace),
        [0xFE, 0xFF, rest @ ..] => UTF_16BE.decode(rest, DecoderTrap::Replace),
        _ => WINDOWS_949.decode(bytes, DecoderTrap::Replace),
    }
    // `DecoderTrap::Replace` never errors; keep the lossy string on the Err arm.
    .unwrap_or_else(|cow| cow.into_owned())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn decodes_utf16le_with_bom() {
        // "Fire" as UTF-16LE preceded by an FF FE BOM.
        let bytes = [0xFF, 0xFE, 0x46, 0x00, 0x69, 0x00, 0x72, 0x00, 0x65, 0x00];
        assert_eq!(decode_textdata(&bytes), "Fire");
    }

    #[test]
    fn decodes_utf16be_with_bom() {
        // "Fire" as UTF-16BE preceded by an FE FF BOM.
        let bytes = [0xFE, 0xFF, 0x00, 0x46, 0x00, 0x69, 0x00, 0x72, 0x00, 0x65];
        assert_eq!(decode_textdata(&bytes), "Fire");
    }

    #[test]
    fn decodes_cp949_without_bom() {
        // "한글" (hangul) in CP949/EUC-KR, no BOM.
        let bytes = [0xC7, 0xD1, 0xB1, 0xDB];
        assert_eq!(decode_textdata(&bytes), "\u{d55c}\u{ae00}");
    }

    #[test]
    fn decodes_ascii_without_bom() {
        // Plain ASCII has no BOM and passes through the CP949 fallback intact.
        assert_eq!(decode_textdata(b"Gold"), "Gold");
    }

    #[test]
    fn decodes_cp949_tab_row() {
        // A tab-separated CP949 row like regioninfo.txt: 1<TAB><hangul><TAB>value.
        let bytes = [
            0x31, 0x09, 0xC1, 0xA6, 0xB8, 0xF1, 0x09, 0x76, 0x61, 0x6C, 0x75, 0x65,
        ];
        assert_eq!(decode_textdata(&bytes), "1\t\u{c81c}\u{baa9}\tvalue");
    }

    #[test]
    fn matches_previous_utf16le_lossy_decode() {
        // Equivalence with the old `chunks(2).skip(1) + from_utf16_lossy` path
        // on real-shaped UTF-16LE content, so migrated consumers see no change.
        let text = "1\tSN_ITEM_ETC_GOLD\t\t\t\t\t\t\t\tGold\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }

        let legacy: Vec<u16> = bytes
            .chunks(2)
            .skip(1)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let legacy = String::from_utf16_lossy(&legacy);

        assert_eq!(decode_textdata(&bytes), legacy);
    }

    #[test]
    fn handles_empty_input() {
        assert_eq!(decode_textdata(&[]), "");
    }
}
