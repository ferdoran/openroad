# Pack File (*.pk2) - JMXPACK

The archive container: a 256-byte header, then a tree of 2,560-byte blocks each
holding twenty 128-byte entries.

Layout derived from openroad's own reader (`bevy_pk2/src/pk2/{header,entry,constants}.rs`),
which parses every archive this project reads. Upstream reference:
`SilkroadDoc.wiki/JMXPACK` —
https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/JMXPACK

## Header (256 bytes, at offset 0)

| offset | size | type | field | notes |
|---|---|---|---|---|
| 0 | 30 | char[30] | `Signature` | `JoyMax File Manager!` plus NUL padding |
| 30 | 4 | u32 LE | `Version` | `0x01000002` |
| 34 | 1 | u8 | `Encrypted` | non-zero ⇒ blocks are Blowfish-encrypted |
| 35 | 16 | u8[16] | `Checksum` | key probe — see below |
| 51 | 205 | u8[205] | `Reserved` | |

When `Encrypted` is set, the reader encrypts the known plaintext
`"Joymax Pak File\0"` with the configured key and compares the **first 3 bytes**
against `Checksum`. That is how a wrong key is detected up front rather than as
garbage entries. The key itself is user-supplied and not compiled in — see
`bevy_pk2/src/pk2/key.rs`.

## Block (2,560 bytes)

Twenty consecutive 128-byte entries. The root block sits at offset 256, directly
after the header. Each block's **20th entry** carries the chain pointer to the
next block of the same directory; a `NextChain` of 0 ends the chain.

## Entry (128 bytes)

| offset | size | type | field | notes |
|---|---|---|---|---|
| 0 | 1 | u8 | `Type` | `0` empty, `1` directory, `2` file |
| 1 | 89 | char[89] | `Name` | CP949, NUL-padded |
| 90 | 8 | u64 LE | `CreateTime` | Windows FILETIME |
| 98 | 8 | u64 LE | `ModifyTime` | Windows FILETIME |
| 106 | 8 | u64 LE | `Position` | file data offset, or child block offset for a directory |
| 114 | 4 | u32 LE | `Size` | file size in bytes |
| 122 | 8 | u64 LE | `NextChain` | next block of this directory, 0 = end |
| 126 | 2 | u8[2] | `Padding` | keeps the entry a whole number of Blowfish blocks |

Encryption is Blowfish in ECB over 8-byte chunks, keyed by the user-supplied key
and salt (`blowfish.rs`, `key.rs`). Because 128 divides evenly by 8, an entry is
directly decryptable on its own.

The reader treats an archive as untrusted input: entry types outside `0..=2`
become `Empty`, and directory chains are bounded (`MAX_CHAIN_BLOCKS`) so a cyclic
`NextChain` is an error rather than an infinite walk.

## Verification + corrections (openroad, 2026-08-12)

Header and entry layout verified byte-exactly against all **5** real archives
(5.6 GB). Every archive agrees: signature `"JoyMax File Manager!\n"` + 9 NULs,
`encrypted = 1`, checksum `d8da30` + **13 zero bytes**, `reserved[205]` all zero.

- **Correction — version endianness.** The on-disk bytes at 0x1E are
  `02 00 00 01`, so the little-endian u32 is **`0x01000002`**, not `0x02000001`
  as the comment above states (that transcribes the byte sequence as if
  big-endian). `bevy_pk2/src/pk2/constants.rs:9` has the correct value.
- **Undocumented-but-deliberate:** `header.rs:84-88` verifies only the **first 3
  bytes** of the checksum. The corpus explains why — real archives store just
  `d8da30` and zero the rest — so this is correct behaviour, not a bug.
- **Undocumented-but-deliberate:** archive lookup is **case-insensitive**
  (`archive.rs:59-68`), because textdata references differ in case from the
  stored names.
- Block chain: only `entries[19].next_chain` is followed (`util.rs:53-55`);
  directory children live at `entry.position`; `"."`/`".."` are filtered by
  `name[0] != 0x2E`. The index is **eager** — the whole tree is flattened into a
  `HashMap` at construction (`archive.rs:88-94`).

## Reader hardening (#295)

The archive reader is our trust boundary — SRO drops are treated as potentially
hostile — so it no longer assumes a well-formed file:

- **Block chains are walked iteratively with a visited-set** and a
  `MAX_CHAIN_BLOCKS` cap; a `next_chain` pointing back into the chain yields
  `Error::ChainLoop` instead of recursing until the stack dies. The **directory
  tree** carries the same guard: a subdirectory whose block was already expanded
  is skipped rather than followed again.
- **`read_exact` replaces `read`.** The old guard only checked that the byte
  count divided evenly by `ENTRY_SIZE` and then parsed all 20 entries out of the
  buffer regardless, so a truncated tail block silently produced entries from
  whatever the buffer still held.
- **The panic surface is gone from the parse paths.** A malformed header, an
  unknown entry-type byte, a missing root directory or a short block are all
  `Error` values now. `Archive::open()` is the fallible entry point;
  `Archive::from()` remains as the startup wrapper, where a missing PK2 really is
  fatal, and panics with one clear message.

