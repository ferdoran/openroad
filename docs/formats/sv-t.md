# SV.T — client version file

A single Blowfish-encrypted block holding the client version as a decimal string,
padded to 1,024 bytes.

Layout derived from openroad's parser (`launcher/src/version.rs`). Upstream
reference: `SilkroadDoc.wiki/SV` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/SV

## Layout

| offset | size | type | field | notes |
|---|---|---|---|---|
| 0 | 4 | u32 LE | `VersionBufferLength` | 8 — one Blowfish block |
| 4 | 8 | u8[] | `VersionBuffer` | encrypted; must be a multiple of 8 |
| 12 | 1012 | u8[] | `FilePadding` | `0x00` |

Total 1,024 bytes.

The key is **not** the archive key and is **not** compiled in — like the PK2 key
it is user-supplied (`pk2.version_key` / `SRO_SVT_KEY`; see
`config.example.yaml`). Only its first 8 bytes reach Blowfish, with a zero salt.

## Byte verification (openroad, 2026-08-12)

`Media/SV.T` is 1,024 B and EOF-exact: `u32 versionBufferLength = 8`, an 8-byte
Blowfish block, then 1,012 bytes of `0x00` padding.

**Correction:** the encrypted block is **not** "4 ascii chars + 4 bytes padding".
Decrypted with the configured version key (first 8 bytes, zero salt), the
plaintext is `33 32 30 38 00 00 00 00` — a **NUL-terminated decimal string**,
here `"208"` (3 chars + 5 NULs). Our launcher's `take_while(|b| **b != 0)`
(`launcher/src/version.rs:37-43`) already handles this correctly; the fixed
4-char reading in the line above does not.

**The client version in this build is 208** — note this is a different number
from the 1.188 data/protocol generation.

Blowfish caveat for anyone reproducing it: SRO reads each 8-byte block as two
**little-endian** u32s (`bevy_pk2/src/pk2/blowfish.rs:62-77`), so stock OpenSSL
`bf-ecb` will not reproduce the plaintext (it also defaults to a 16-byte key).
The zero-salt path is the identity XOR at `blowfish.rs:134-143`.
