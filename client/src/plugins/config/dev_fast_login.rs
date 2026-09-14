use serde::Deserialize;

/// Optional developer fast-login. When `enabled`, the `intro_v2` scene logs in
/// with these credentials and joins the first character with no manual steps
/// (see `scenes/intro_v2/dev_fast_login.rs`). The headless net-check client
/// (`netcheck.rs`) reuses `username`/`password` regardless of `enabled`.
///
/// **Not the original's "Auto Login".** v1.188 does ship a feature by that name
/// — `textuisystem.txt:233-234` (`UIO_MSG_AUTOLOGIN`, `UIO_STT_AUTOLOGIN`) — but
/// it is a **login queue**: when a shard is full the client refuses login and
/// retries on a timer with a user cancel. It stores no credential, has no
/// toggle and no UI tree, and it is **not implemented here**
/// (`docs/re/ui/scene-intro-autologin.md`). This block is openroad-only dev
/// convenience and is named so the two cannot be confused; the old `autologin:`
/// key still deserializes so existing `config.yaml` files keep working.
#[derive(Deserialize, Debug, Default)]
pub struct DevFastLoginSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// The IBUV image code to answer the 0x2322 challenge with.
    ///
    /// There is no sane default: the code is an *image* the server generates,
    /// so only a server whose captcha is constant can be answered blind. When
    /// this is unset the fast-login path leaves the challenge to the normal
    /// captcha modal instead of guessing (which is what the previously
    /// hardcoded `"1"` did — it could not survive a real challenge).
    #[serde(default)]
    pub captcha_answer: Option<String>,
}
