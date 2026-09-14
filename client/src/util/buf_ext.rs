use std::path::{PathBuf, MAIN_SEPARATOR_STR};

use bevy::prelude::{Color, Mat4, Vec2, Vec3, Vec4};
use bytes::{Buf, Bytes};
use encoding::all::WINDOWS_949;
use encoding::{DecoderTrap, Encoding};

/// Decode CP949 (WHATWG euc-kr) bytes, replacing invalid sequences rather than
/// failing. Same table `bevy_pk2` uses for archive filenames and
/// `assets::textdata::decode` uses for BOM-less tables, so a path read out of a
/// JMX field and the archive entry it names now agree.
pub fn decode_cp949(bytes: &[u8]) -> String {
    WINDOWS_949
        .decode(bytes, DecoderTrap::Replace)
        .unwrap_or_else(|cow| cow.into_owned())
}

pub trait BufExt: Buf {
    fn get_string(&mut self) -> String;
    fn get_double_len_string(&mut self) -> String;
    fn get_fixed_size_string(&mut self, size: usize) -> String;
    fn get_vec2(&mut self) -> Vec2;
    fn get_vec3(&mut self) -> Vec3;
    fn get_vec4(&mut self) -> Vec4;
    fn get_mat4(&mut self) -> Mat4;
    fn get_path_buf(&mut self) -> PathBuf;
    fn get_path_buf_double_len(&mut self) -> PathBuf;
    fn get_color_rgba(&mut self) -> Color;
}

impl<T: Buf> BufExt for T {
    fn get_string(&mut self) -> String {
        let str_len = self.get_u16_le();
        self.get_fixed_size_string(str_len as usize)
    }

    fn get_double_len_string(&mut self) -> String {
        let str_len = self.get_u32_le();
        self.get_fixed_size_string(str_len as usize)
    }

    /// Read a fixed-size, NUL-terminated CP949 field.
    ///
    /// SRO's JMX string fields are CP949 (the encoding `bevy_pk2` already uses
    /// for archive filenames), so decoding them latin1-and-`unidecode` turned
    /// real Korean text into transliterated garbage: all 2,206 non-ASCII
    /// strings in the `.2dt` corpus, 3 `.ban` animation names, and 9 `.bmt`
    /// material names and `diffuse_map_path`s — the last of which made those
    /// materials load no texture at all. CP949's single-byte range is ASCII, so
    /// this is a no-op for the ASCII-only callers (`.bsr`, `.bsk`, `.bms`,
    /// `.cpd`, every 12-byte signature).
    ///
    /// Truncating at the first NUL is what retires the old "strip every `0xFD`
    /// byte" hack: `0xFD` is a stale exporter fill byte *after* the terminator
    /// in 7 corpus files, but also a legal CP949 lead byte inside real text, so
    /// stripping it everywhere corrupted the text it was meant to clean up.
    fn get_fixed_size_string(&mut self, size: usize) -> String {
        // Clamp rather than slice-panic: a zero-byte or truncated file (4 empty
        // `.bsk` files ship in the corpus) used to blow up here, before the
        // caller's signature check could turn it into an error. Clamped against
        // `chunk()` rather than `remaining()` so a non-contiguous `Buf` cannot
        // index past the current chunk either.
        let take = size.min(self.chunk().len());
        let field = Bytes::copy_from_slice(&self.chunk()[..take]);
        self.advance(take);
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        decode_cp949(&field[..end])
    }

    fn get_vec2(&mut self) -> Vec2 {
        Vec2::new(self.get_f32_le(), self.get_f32_le())
    }

    fn get_vec3(&mut self) -> Vec3 {
        Vec3::new(self.get_f32_le(), self.get_f32_le(), self.get_f32_le())
    }

    fn get_vec4(&mut self) -> Vec4 {
        Vec4::new(
            self.get_f32_le(),
            self.get_f32_le(),
            self.get_f32_le(),
            self.get_f32_le(),
        )
    }

    fn get_mat4(&mut self) -> Mat4 {
        Mat4::from_cols(
            self.get_vec4(),
            self.get_vec4(),
            self.get_vec4(),
            self.get_vec4(),
        )
    }

    fn get_path_buf(&mut self) -> PathBuf {
        let s = self
            .get_string()
            .replace(r"\\", r"\")
            .replace(r"\", MAIN_SEPARATOR_STR);
        PathBuf::from(s)
    }

    fn get_path_buf_double_len(&mut self) -> PathBuf {
        let s = self
            .get_double_len_string()
            .replace(r"\\", r"\")
            .replace(r"\", MAIN_SEPARATOR_STR);
        PathBuf::from(s)
    }

    fn get_color_rgba(&mut self) -> Color {
        let r = self.get_f32_le();
        let g = self.get_f32_le();
        let b = self.get_f32_le();
        let a = self.get_f32_le();

        Color::srgba(r, g, b, a)
    }
}

#[cfg(test)]
mod test {
    use bytes::Bytes;

    use crate::util::buf_ext::BufExt;

    #[test]
    fn read_fixed_size_64() {
        const INPUT: &str = "45Kzvmdq46x4U01iKvsIdOtMTgImXtImi49i5DYik5lli47WxkxWRFiLdMuNFvrO";
        let expected = INPUT.to_string();
        let mut bytes = Bytes::copy_from_slice(INPUT.as_bytes());

        let result = bytes.get_fixed_size_string(64);
        assert_eq!(result, expected);
    }

    #[test]
    fn read_fixed_size_128() {
        const INPUT: &str = "dHZ1ETYoVarBpDXLcxx5Bol2OkGWEx4pnU3V0NQO8PDmJ9CRWCZAg52wq4VIT3OJhDqYWVQvGQLGfQaYUIU8xThqLVxGw8ngSIOUXX9EKetDYj1V4lU6z91WIp0SlMyj";
        let expected = INPUT.to_string();
        let mut bytes = Bytes::copy_from_slice(INPUT.as_bytes());

        let result = bytes.get_fixed_size_string(128);
        assert_eq!(result, expected);
    }

    #[test]
    fn read_fixed_size_256() {
        const INPUT: &str = "COmzIPdupYykGkJoIUJywvLFgjcuGuUL8Qm2vdIJbrYolTtccQIA4XZExBzgJOmwcWG3y7C7L33iLLZsgDY4gJguYI4F51pBEXOQ7ms9Au1ND1ROU1F5L6I7UAACLS1IKhv8SImDJzQUWzB9lLJJ1qdIWAUjszhgdofzddUk88Bu8Gydx3s2SXKEMNUocJlzsR6SCs2blcBklz7ZEiwQJ1tkpxnpoogHH2lIgKDed1puvur1HuMdCgdLw33Deyya";
        let expected = INPUT.to_string();
        let mut bytes = Bytes::copy_from_slice(INPUT.as_bytes());

        let result = bytes.get_fixed_size_string(256);
        assert_eq!(result, expected);
    }

    /// #294: JMX string fields are CP949. Read latin1-then-`unidecode` they
    /// became transliterated garbage — all 2,206 non-ASCII `.2dt` strings, 3
    /// `.ban` animation names, and 9 `.bmt` material names and
    /// `diffuse_map_path`s. The last of those is a live rendering bug: a
    /// mangled path loads no texture.
    #[test]
    fn decodes_a_cp949_field_instead_of_transliterating_it() {
        let mut field = vec![0xB9, 0xDA, 0xBD, 0xBA]; // CP949 "박스"
        field.resize(64, 0);
        let mut bytes = Bytes::copy_from_slice(&field);

        assert_eq!(bytes.get_fixed_size_string(64), "박스");
    }

    /// CP949's single-byte range is ASCII, so every ASCII-only caller (`.bsr`,
    /// `.bsk`, `.bms`, `.cpd`, and every 12-byte signature) is unaffected.
    #[test]
    fn an_ascii_signature_is_unchanged() {
        let mut bytes = Bytes::copy_from_slice(b"JMXVBSR 0110");

        assert_eq!(bytes.get_fixed_size_string(12), "JMXVBSR 0110");
    }

    /// A field shorter than its declared size must not slice out of bounds.
    #[test]
    fn a_short_field_does_not_panic() {
        let mut bytes = Bytes::from_static(&[0x41, 0x42]);

        assert_eq!(bytes.get_fixed_size_string(64), "AB");
    }
}
