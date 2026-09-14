use std::io;

#[derive(std::fmt::Debug)]
pub enum Error {
    InvalidHeader(&'static str),
    IO(io::Error),
    InvalidBlock(&'static str),
    /// The block chain revisited an offset, or ran past the block cap. A
    /// hostile or corrupt archive can point a block's `next_chain` back at
    /// itself; without this the reader recurses until the stack dies.
    ChainLoop(u64),
}
