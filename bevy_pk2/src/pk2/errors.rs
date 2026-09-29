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
    /// The directory tree contains more directories than
    /// [`crate::pk2::constants::MAX_DIRECTORIES`]. A full tree is not a cycle:
    /// this is the backstop against an archive that keeps handing out fresh,
    /// valid directory blocks forever, and it carries the cap that was hit.
    TooManyDirectories(usize),
}

/// One readable line per failure. `Debug` would put
/// `IO(Os { code: 2, ... })` in front of a user whose only real problem is that
/// a file is not where the client looked.
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidHeader(what) => write!(f, "not a readable PK2 archive: {what}"),
            Error::IO(err) if err.kind() == io::ErrorKind::NotFound => write!(f, "file not found"),
            Error::IO(err) => write!(f, "cannot be read: {err}"),
            Error::InvalidBlock(what) => write!(f, "malformed archive block: {what}"),
            Error::ChainLoop(offset) => {
                write!(f, "block chain loops or never ends at offset {offset}")
            }
            Error::TooManyDirectories(cap) => {
                write!(f, "archive holds more than {cap} directories")
            }
        }
    }
}

/// `Error` crosses a crate boundary, so it has to be an error in the language's
/// sense too: without this impl a caller cannot put it in a `Box<dyn Error>`,
/// use `?` into `anyhow`/`eyre`, or ask for the `source()` of an I/O failure.
/// Only the [`Error::IO`] variant wraps another error; the rest are terminal.
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::IO(err) => Some(err),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trait impl is the point: a caller two crates away holds this as a
    /// `Box<dyn std::error::Error>`, and reaches the underlying `io::Error`
    /// through `source()` rather than by matching our variants.
    #[test]
    fn the_error_is_a_std_error_and_names_its_io_source() {
        let err = Error::IO(io::Error::from(io::ErrorKind::NotFound));
        let boxed: Box<dyn std::error::Error> = Box::new(err);
        assert_eq!(boxed.to_string(), "file not found");
        let source = boxed.source().expect("an I/O failure carries its source");
        assert!(
            source.downcast_ref::<io::Error>().is_some(),
            "the source is the io::Error itself"
        );

        let terminal: Box<dyn std::error::Error> = Box::new(Error::ChainLoop(2560));
        assert!(terminal.source().is_none(), "a chain loop wraps nothing");
        assert_eq!(
            terminal.to_string(),
            "block chain loops or never ends at offset 2560"
        );
    }
}
