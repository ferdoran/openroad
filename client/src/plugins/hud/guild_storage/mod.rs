pub mod model;

use bevy::prelude::*;

/// Self-registration for the guild-storage consumer (#558). Model only: the
/// guild warehouse has no window yet (`GDR_GUILDSTORAGEROOM` is not ported),
/// so this plugin owns the session state and the wire round trip, and the
/// only player-visible output is the refusal path.
pub struct GuildStoragePlugin;

impl Plugin for GuildStoragePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<model::GuildStorageState>()
            .init_resource::<model::GuildStorageDataBuffer>()
            .add_systems(
                Update,
                (
                    model::open_guild_storage,
                    model::on_guild_storage_response,
                    model::close_guild_storage_with_dialog,
                    // chained for the same reason the personal storage push
                    // is: begin/chunk/end usually land in ONE frame, and an
                    // unordered tuple lets `end` parse an empty buffer
                    (
                        model::on_guild_storage_begin,
                        model::on_guild_storage_chunk,
                        model::on_guild_storage_end,
                    )
                        .chain(),
                )
                    .run_if(super::hud_scenes),
            );
    }
}
