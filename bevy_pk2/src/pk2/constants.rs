pub const ENTRY_SIZE: usize = 128;
pub const BLOCK_SIZE: usize = 20 * ENTRY_SIZE;
pub const HEADER_SIZE: usize = 256;
// The archive key and its salt are deliberately NOT here. They are supplied by
// the user at runtime through local configuration and carried in
// [`crate::Pk2Key`] — see `key.rs` for why. The constants below are container
// format identifiers, not key material: they are what a PK2 *is*, and are
// needed to recognize and parse one at all.
pub const CHECKSUM: &[u8; 16] = b"Joymax Pak File\0";
pub const SIGNATURE: &[u8; 30] = b"JoyMax File Manager!\x0a\x00\x00\x00\x00\x00\x00\x00\x00\x00";
pub const VERSION: u32 = 0x0100_0002;
/// Entries per block — the 20th carries the chain pointer.
pub const ENTRIES_PER_BLOCK: usize = BLOCK_SIZE / ENTRY_SIZE;
/// Hard cap on how many blocks one directory chain may span. The shipped
/// archives' longest chain is far below this; it exists so a hostile or
/// corrupt `next_chain` cannot spin forever even when every offset is unique.
pub const MAX_CHAIN_BLOCKS: usize = 4096;
