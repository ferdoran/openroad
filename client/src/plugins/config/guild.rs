//! Guild-window knobs.

use serde::Deserialize;

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct GuildSettings {
    /// Draw `GDR_GUILD_COMMAND_BUTTON_6` (`UIIT_CTL_GUILD_POSITION_GRANT`,
    /// "Position allocating") instead of `_5` (`UIIT_STT_GUILD_NAME_GRANT`).
    ///
    /// The two buttons share the rect `353,250,0,0` **byte-for-byte**
    /// (`ifguild.txt:507,:488`), so exactly one is ever drawn — the data
    /// encodes the choice as a geometric collision and gives no rule for
    /// picking. Button 6 is the fortress-era surface and carries `Style=64`
    /// while every sibling carries `0`; `docs/re/ui/hud-guild-window.md` §3.7 /
    /// §9-U5 leaves the mechanism `[U]`. So the default is button 5 and this
    /// flag is the escape hatch for a server that runs the position-grant era.
    pub position_grant: bool,
}
