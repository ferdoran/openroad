use serde_derive::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SceneSettings {
    pub intro_location: String,
    pub char_select_location: String,
    pub startup: String,
}

impl Default for SceneSettings {
    fn default() -> Self {
        Self {
            // Empty means "whatever your own config/option.txt names" — the
            // cutscene is read from your Media.pk2 at startup, so there is no
            // name for us to default to that is better than the client's own.
            intro_location: String::new(),
            char_select_location: String::from("constantinople"),
            startup: String::from("world"),
        }
    }
}
