use std::collections::HashSet;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};

use bytes::Bytes;

use crate::pk2::blowfish::Blowfish;
use crate::pk2::constants::{BLOCK_SIZE, ENTRIES_PER_BLOCK, ENTRY_SIZE, MAX_CHAIN_BLOCKS};
use crate::pk2::entry::Entry;
use crate::pk2::errors::Error;
use crate::pk2::errors::Error::{InvalidBlock, IO};

/// Converts a byte slice in little endian to an u32 number.
pub fn as_u32_le(array: &[u8]) -> u32 {
    ((array[0] as u32) << 0)
        + ((array[1] as u32) << 8)
        + ((array[2] as u32) << 16)
        + ((array[3] as u32) << 24)
}

/// Converts a byte slice in little endian to an u64 number.
pub fn as_u64_le(array: &[u8]) -> u64 {
    ((array[0] as u64) << 0)
        + ((array[1] as u64) << 8)
        + ((array[2] as u64) << 16)
        + ((array[3] as u64) << 24)
        + ((array[4] as u64) << 32)
        + ((array[5] as u64) << 40)
        + ((array[6] as u64) << 48)
        + ((array[7] as u64) << 56)
}

/// Decrypt one already-read block buffer into its entries.
fn decode_block(entry_buf: &mut [u8; BLOCK_SIZE], blowfish: &Blowfish) -> Vec<Entry> {
    blowfish.decrypt(entry_buf);
    entry_buf
        .chunks_exact(ENTRY_SIZE)
        .map(Entry::from)
        .collect()
}

/// The next block offset in the chain, or `None` at the end. The chain pointer
/// lives on the block's last entry.
fn next_chain(entries: &[Entry]) -> Option<u64> {
    entries
        .get(ENTRIES_PER_BLOCK - 1)
        .map(|e| e.next_chain)
        .filter(|next| *next > 0)
}

/// Guards a chain walk: rejects a revisited offset or an over-long chain.
///
/// The archive is our trust boundary — SRO drops are treated as potentially
/// hostile — so a cyclic `next_chain` has to be an error, not an infinite
/// recursion that exhausts the stack.
#[derive(Default)]
struct ChainGuard {
    visited: HashSet<u64>,
}

impl ChainGuard {
    fn visit(&mut self, offset: u64) -> Result<(), Error> {
        if self.visited.len() >= MAX_CHAIN_BLOCKS || !self.visited.insert(offset) {
            return Err(Error::ChainLoop(offset));
        }
        Ok(())
    }
}

/// Reads a block (see [BLOCK_SIZE]) in a PK2 archive and returns the entries,
/// following the chain iteratively.
///
/// `read_exact` rather than `read`: the old short-read guard only checked that
/// the byte count divided evenly by [`ENTRY_SIZE`], then parsed all 20 entries
/// out of the buffer regardless — so a truncated tail block silently produced
/// entries from whatever the rest of the buffer still held.
pub fn read_block(file: &mut File, offset: u64, blowfish: &Blowfish) -> Result<Vec<Entry>, Error> {
    let mut guard = ChainGuard::default();
    let mut entries: Vec<Entry> = Vec::new();
    let mut next = Some(offset);

    while let Some(offset) = next {
        guard.visit(offset)?;
        let mut entry_buf: [u8; BLOCK_SIZE] = [0; BLOCK_SIZE];
        file.seek(SeekFrom::Start(offset)).map_err(IO)?;
        file.read_exact(&mut entry_buf)
            .map_err(|_| InvalidBlock("short block"))?;
        let block = decode_block(&mut entry_buf, blowfish);
        next = next_chain(&block);
        entries.extend(block);
    }

    entries.retain(|e| !e.is_empty());
    Ok(entries)
}

pub trait CursorExt {
    fn read_block(&mut self, offset: u64, blowfish: &Blowfish) -> Result<Vec<Entry>, Error>;
}

impl CursorExt for Cursor<Bytes> {
    /// Same bounded walk as [`read_block`], over an in-memory archive.
    fn read_block(&mut self, offset: u64, blowfish: &Blowfish) -> Result<Vec<Entry>, Error> {
        let mut guard = ChainGuard::default();
        let mut entries: Vec<Entry> = Vec::new();
        let mut next = Some(offset);

        while let Some(offset) = next {
            guard.visit(offset)?;
            self.seek(SeekFrom::Start(offset)).map_err(IO)?;
            let mut entry_buf: [u8; BLOCK_SIZE] = [0; BLOCK_SIZE];
            self.read_exact(&mut entry_buf)
                .map_err(|_| InvalidBlock("short block"))?;
            let block = decode_block(&mut entry_buf, blowfish);
            next = next_chain(&block);
            entries.extend(block);
        }

        entries.retain(|e| !e.is_empty());
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pk2::constants::MAX_CHAIN_BLOCKS;
    use crate::pk2::key::Pk2Key;

    /// These tests encrypt and decrypt with the same cipher, so any valid key
    /// exercises the block-chain logic they are actually about. They use a
    /// fixture rather than the real archive key, which is user-supplied and
    /// deliberately absent from this crate.
    fn test_cipher() -> Blowfish {
        let key = Pk2Key::from_config("testkey", "00112233445566778899").unwrap();
        Blowfish::from_key(&key).expect("fixture key is valid")
    }

    /// Build one encrypted block whose 20th entry chains to `next`.
    fn block(next: u64, blowfish: &Blowfish) -> Vec<u8> {
        let mut buf = [0u8; BLOCK_SIZE];
        // entry 0: a file entry so the block is not filtered away entirely
        buf[0] = 2;
        buf[1] = b'a';
        // entry 19's next_chain lives at byte 118 of its own 128-byte slot
        let last = (ENTRIES_PER_BLOCK - 1) * ENTRY_SIZE;
        buf[last] = 1; // dir, so it survives is_empty()
        buf[last + 118..last + 126].copy_from_slice(&next.to_le_bytes());
        blowfish.encrypt(&mut buf);
        buf.to_vec()
    }

    /// A block chaining to itself used to recurse until the stack died. The
    /// archive is untrusted input, so it has to be an error instead.
    ///
    /// The block sits at offset `BLOCK_SIZE`, not 0, because `next_chain == 0`
    /// is the end-of-chain sentinel — a "cycle" back to offset 0 is simply
    /// termination and cannot be expressed.
    #[test]
    fn a_self_referential_chain_is_rejected() {
        let blowfish = test_cipher();
        let one = BLOCK_SIZE as u64;
        let mut bytes = vec![0u8; BLOCK_SIZE]; // filler so offset 1 is real
        bytes.extend_from_slice(&block(one, &blowfish));
        let mut cursor = Cursor::new(Bytes::from(bytes));
        assert!(matches!(
            cursor.read_block(one, &blowfish),
            Err(Error::ChainLoop(o)) if o == one
        ));
    }

    /// Two blocks pointing at each other is the same hazard one step removed.
    #[test]
    fn a_two_block_cycle_is_rejected() {
        let blowfish = test_cipher();
        let (one, two) = (BLOCK_SIZE as u64, 2 * BLOCK_SIZE as u64);
        let mut bytes = vec![0u8; BLOCK_SIZE];
        bytes.extend_from_slice(&block(two, &blowfish)); // at `one` -> `two`
        bytes.extend_from_slice(&block(one, &blowfish)); // at `two` -> `one`
        let mut cursor = Cursor::new(Bytes::from(bytes));
        assert!(matches!(
            cursor.read_block(one, &blowfish),
            Err(Error::ChainLoop(_))
        ));
    }

    /// A truncated tail block must be an error, not 20 entries parsed out of
    /// whatever the buffer still held.
    #[test]
    fn a_short_block_is_rejected() {
        let blowfish = test_cipher();
        let mut bytes = block(BLOCK_SIZE as u64, &blowfish);
        bytes.extend_from_slice(&[0u8; 64]); // second block cut short
        let mut cursor = Cursor::new(Bytes::from(bytes));
        assert!(matches!(
            cursor.read_block(0, &blowfish),
            Err(Error::InvalidBlock(_))
        ));
    }

    /// A well-formed terminating chain still reads normally.
    #[test]
    fn a_terminated_chain_reads_its_entries() {
        let blowfish = test_cipher();
        let bytes = Bytes::from(block(0, &blowfish));
        // patch: no chain at all -> next_chain 0 means "end"
        let mut plain = [0u8; BLOCK_SIZE];
        plain[0] = 2;
        plain[1] = b'a';
        blowfish.encrypt(&mut plain);
        let mut cursor = Cursor::new(Bytes::from(plain.to_vec()));
        let entries = cursor.read_block(0, &blowfish).expect("reads");
        assert_eq!(entries.len(), 1, "one non-empty entry");
        assert!(entries[0].is_file());
        drop(bytes);
    }

    /// The cap is a backstop for a chain of unique offsets that never ends.
    #[test]
    fn the_block_cap_is_bounded() {
        assert!(MAX_CHAIN_BLOCKS >= 1024, "must fit real archives");
        assert!(MAX_CHAIN_BLOCKS <= 1 << 20, "must still bound the walk");
    }
}
