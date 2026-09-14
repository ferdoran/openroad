//! The archive key, supplied by the user at runtime and never compiled in.
//!
//! IDEA. A PK2 is encrypted, so reading one means holding the key that opens
//! it. Baking that key into our binary would make the shipped artifact carry
//! the means of decrypting the archive — so instead it arrives from the user's
//! own local configuration, alongside the archives it belongs to, and an
//! archive simply cannot be opened without one. There is deliberately no
//! default and no fallback: a missing key is an error the caller must report,
//! not something this crate quietly fills in.
//!
//! [`Pk2Key`] owns the key material and the salt that [`crate::Blowfish`]
//! folds into it, and redacts itself in `Debug` so a stray log line or panic
//! message cannot print it.

use std::fmt;
use std::path::{Path, PathBuf};

/// Environment overrides, checked before the config file.
pub const KEY_ENV: &str = "SRO_PK2_KEY";
pub const SALT_ENV: &str = "SRO_PK2_SALT";
/// The launcher's SV.T version file uses its own, different key.
pub const VERSION_KEY_ENV: &str = "SRO_SVT_KEY";
pub const VERSION_SALT_ENV: &str = "SRO_SVT_SALT";
/// Resolved relative to the working directory, matching how the client's
/// `ClientConfig` loads the same file (`config::File::with_name("config")`).
pub const CONFIG_FILE: &str = "config.yaml";

/// Blowfish's key schedule accepts 4..=56 bytes; the salt is XORed over the
/// first `key.len()` of them, so a salt longer than the key is simply unused.
pub const MIN_KEY_LEN: usize = 4;
pub const MAX_KEY_LEN: usize = 56;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// Outside Blowfish's accepted key length.
    InvalidLength(usize),
    /// The salt was longer than the key schedule's working buffer.
    SaltTooLong(usize),
    /// The salt string was not valid hexadecimal.
    MalformedSaltHex,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::InvalidLength(n) => write!(
                f,
                "PK2 key must be {MIN_KEY_LEN}..={MAX_KEY_LEN} bytes, got {n}"
            ),
            KeyError::SaltTooLong(n) => {
                write!(f, "PK2 salt must be at most {MAX_KEY_LEN} bytes, got {n}")
            }
            KeyError::MalformedSaltHex => {
                write!(f, "PK2 salt must be an even-length hexadecimal string")
            }
        }
    }
}

impl std::error::Error for KeyError {}

/// Key material for one archive family. Cheap to clone; every [`Archive`]
/// keeps the derived cipher rather than this.
///
/// [`Archive`]: crate::prelude::Archive
#[derive(Clone, PartialEq, Eq)]
pub struct Pk2Key {
    key: Vec<u8>,
    salt: Vec<u8>,
}

impl Pk2Key {
    pub fn new(key: impl Into<Vec<u8>>, salt: impl Into<Vec<u8>>) -> Result<Self, KeyError> {
        let key = key.into();
        let salt = salt.into();
        if key.len() < MIN_KEY_LEN || key.len() > MAX_KEY_LEN {
            return Err(KeyError::InvalidLength(key.len()));
        }
        if salt.len() > MAX_KEY_LEN {
            return Err(KeyError::SaltTooLong(salt.len()));
        }
        Ok(Pk2Key { key, salt })
    }

    /// Build from the two strings local configuration carries: the key as
    /// plain text (it is an ASCII digit string in every archive we have seen)
    /// and the salt as hex, which is how a byte array is least ambiguous to
    /// write in YAML.
    pub fn from_config(key: &str, salt_hex: &str) -> Result<Self, KeyError> {
        Pk2Key::new(key.as_bytes(), decode_hex(salt_hex)?)
    }

    pub fn key(&self) -> &[u8] {
        &self.key
    }

    pub fn salt(&self) -> &[u8] {
        &self.salt
    }
}

/// Redacted on purpose: this type ends up inside `Archive`, which is a bevy
/// `Resource`, and an inspector dump or a panic message must not print it.
impl fmt::Debug for Pk2Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pk2Key")
            .field("key", &format_args!("<redacted, {} bytes>", self.key.len()))
            .field(
                "salt",
                &format_args!("<redacted, {} bytes>", self.salt.len()),
            )
            .finish()
    }
}

/// Why a key could not be assembled. [`fmt::Display`] spells out both supply
/// mechanisms, because this error is the only thing a user with no key
/// configured will ever see.
#[derive(Debug)]
pub enum ResolveError {
    /// Neither the environment nor the config file supplied a key.
    NotConfigured { looked_in: PathBuf },
    /// A key was found but no salt to go with it (or vice versa).
    Incomplete(&'static str),
    /// The config file exists but could not be read or parsed.
    Config(String),
    /// The values were found but are not usable key material.
    Invalid(KeyError),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::NotConfigured { looked_in } => write!(
                f,
                "no PK2 key configured.\n\
                 The archive key is not shipped with this program — supply your own, either by\n\
                   * setting {KEY_ENV} and {SALT_ENV} in the environment, or\n\
                   * adding a `pk2:` block with `key:` and `salt:` to {}\n\
                 See config.example.yaml for the expected shape.",
                looked_in.display()
            ),
            ResolveError::Incomplete(what) => write!(
                f,
                "incomplete PK2 key configuration: {what}.\n\
                 Both the key and its salt are required; see config.example.yaml."
            ),
            ResolveError::Config(e) => write!(f, "could not read {CONFIG_FILE}: {e}"),
            ResolveError::Invalid(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ResolveError {}

impl From<KeyError> for ResolveError {
    fn from(e: KeyError) -> Self {
        ResolveError::Invalid(e)
    }
}

impl Pk2Key {
    /// Assemble the key from the user's own environment: `SRO_PK2_KEY` /
    /// `SRO_PK2_SALT` first, then a `pk2:` block in `config.yaml`. Each field
    /// falls back independently, so the environment can override just one.
    ///
    /// There is no built-in default at any step — if nothing supplies a key
    /// this returns [`ResolveError::NotConfigured`], whose message names both
    /// mechanisms.
    pub fn resolve() -> Result<Self, ResolveError> {
        Pk2Key::resolve_from(Path::new(CONFIG_FILE))
    }

    /// [`Pk2Key::resolve`] against an explicit config path, so tests need not
    /// depend on the working directory.
    pub fn resolve_from(config_path: &Path) -> Result<Self, ResolveError> {
        let section = read_config_section(config_path)?;
        resolve_pair(
            config_path,
            KEY_ENV,
            SALT_ENV,
            section.as_ref().and_then(|s| s.key.clone()),
            section.as_ref().and_then(|s| s.salt.clone()),
        )
    }

    /// The launcher's SV.T version-file key, which is a *different* key from
    /// the archive one and lives under its own config entries. Same supply
    /// rules: environment first, then `config.yaml`, never a built-in value.
    pub fn resolve_version() -> Result<Self, ResolveError> {
        Pk2Key::resolve_version_from(Path::new(CONFIG_FILE))
    }

    pub fn resolve_version_from(config_path: &Path) -> Result<Self, ResolveError> {
        let section = read_config_section(config_path)?;
        resolve_pair(
            config_path,
            VERSION_KEY_ENV,
            VERSION_SALT_ENV,
            section.as_ref().and_then(|s| s.version_key.clone()),
            section.as_ref().and_then(|s| s.version_salt.clone()),
        )
    }
}

/// Environment-overrides-config for one key/salt pair, shared by both keys so
/// they cannot drift apart in precedence or error reporting.
fn resolve_pair(
    config_path: &Path,
    key_env: &str,
    salt_env: &str,
    key_from_file: Option<String>,
    salt_from_file: Option<String>,
) -> Result<Pk2Key, ResolveError> {
    let from_env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let key = from_env(key_env).or(key_from_file);
    let salt = from_env(salt_env).or(salt_from_file);

    match (key, salt) {
        (Some(k), Some(s)) => Ok(Pk2Key::from_config(&k, &s)?),
        (None, None) => Err(ResolveError::NotConfigured {
            looked_in: config_path.to_path_buf(),
        }),
        (Some(_), None) => Err(ResolveError::Incomplete("a key with no salt")),
        (None, Some(_)) => Err(ResolveError::Incomplete("a salt with no key")),
    }
}

/// The `pk2:` block, as far as this crate cares about it. Everything else in
/// the file is ignored, so this stays decoupled from the client's own config
/// struct.
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct Pk2Section {
    pub key: Option<String>,
    pub salt: Option<String>,
    pub version_key: Option<String>,
    pub version_salt: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ConfigFile {
    pk2: Option<Pk2Section>,
}

/// A missing config file is not an error — the environment may carry the key.
fn read_config_section(path: &Path) -> Result<Option<Pk2Section>, ResolveError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ResolveError::Config(e.to_string())),
    };
    let parsed: ConfigFile =
        serde_yaml::from_str(&text).map_err(|e| ResolveError::Config(e.to_string()))?;
    Ok(parsed.pk2)
}

fn decode_hex(s: &str) -> Result<Vec<u8>, KeyError> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err(KeyError::MalformedSaltHex);
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| KeyError::MalformedSaltHex))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_salt_round_trips() {
        let k = Pk2Key::from_config("abcdef", "0A0BFF").unwrap();
        assert_eq!(k.salt(), &[0x0A, 0x0B, 0xFF]);
        assert_eq!(k.key(), b"abcdef");
    }

    #[test]
    fn empty_salt_is_allowed() {
        assert!(Pk2Key::from_config("abcdef", "").unwrap().salt().is_empty());
    }

    #[test]
    fn rejects_short_key() {
        assert_eq!(
            Pk2Key::from_config("abc", "00"),
            Err(KeyError::InvalidLength(3))
        );
    }

    #[test]
    fn rejects_malformed_salt() {
        assert_eq!(
            Pk2Key::from_config("abcdef", "0A0"),
            Err(KeyError::MalformedSaltHex)
        );
        assert_eq!(
            Pk2Key::from_config("abcdef", "zz"),
            Err(KeyError::MalformedSaltHex)
        );
    }

    /// The whole point of the type: a key must never reach a log line.
    #[test]
    fn debug_does_not_leak_key_material() {
        let rendered = format!("{:?}", Pk2Key::from_config("sekret1", "DEAD").unwrap());
        assert!(!rendered.contains("sekret1"));
        assert!(!rendered.contains("DEAD"));
        assert!(rendered.contains("redacted"));
    }
}
