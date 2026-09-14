use bevy::animation::graph::AnimationGraphHandle;
use bevy::animation::AnimationPlayer;
use bevy::app::{App, Plugin};
use bevy::asset::Assets;
use bevy::input::ButtonInput;
use bevy::prelude::{
    in_state, AnimationClip, AnimationGraph, Commands, Entity, Handle, IntoScheduleConfigs,
    KeyCode, Name, OnEnter, Query, Res, ResMut, Resource, Transform, Update, With, Without,
};
use bevy_asset_loader::prelude::{
    AssetCollection, ConfigureLoadingState, LoadingStateAppExt, LoadingStateConfig,
};

use crate::assets::ban::JMXVBAN;
use crate::assets::bsk::JMXVBSK;
use crate::assets::bsr::resource::SroResource;
use crate::commands::SilkroadEntity;
use crate::plugins::camera::spawn_player_camera;
use crate::scenes::SceneState;
use crate::util::commands_ext::CommandsExt;

pub struct NewAssetLoadingScenePlugin;

#[derive(AssetCollection, Resource)]
pub struct AssetsToLoad {
    // #[asset(path = "data://res/npc/npc/chinaetc_kisaeng6.bsr")]
    // #[asset(path = "data://res/mob/oasis/blackrobber.bsr")]
    // #[asset(path = "data://res/nature/common/tree/tre_maple03_big.bsr")]
    #[asset(path = "data://res/nature/common/tree/tre_maple03.bsr")]
    pub resource: Handle<SroResource>,

    // #[asset(path = "data://prim/ani/npc/china/chinaetc_kisaeng6_time.ban")]
    // #[asset(path = "data://prim/ani/mob/oasis/blackrobber/blackrobber_attack02.ban")]
    // #[asset(path = "data://prim/ani/nature/common/tree/tre_maple03_big.ban")]
    #[asset(path = "data://prim/ani/nature/common/tree/tre_maple03.ban")]
    pub animation: Handle<JMXVBAN>,
}

impl Plugin for NewAssetLoadingScenePlugin {
    fn build(&self, app: &mut App) {
        app.configure_loading_state(
            LoadingStateConfig::new(SceneState::Loading).load_collection::<AssetsToLoad>(),
        )
        .add_systems(
            OnEnter(SceneState::AssetLoadTesting),
            (spawn_player_camera, on_loaded),
        )
        .add_systems(
            Update,
            add_animation.run_if(in_state(SceneState::AssetLoadTesting)),
        );
    }
}

fn on_loaded(mut commands: Commands, loaded_assets: Res<AssetsToLoad>) {
    for x in 0..10 {
        for z in 0..10 {
            let x = (x * 150) as f32;
            let z = (z * 150) as f32;
            let transform = Transform::from_xyz(x, 0.0, z);
            commands.spawn_resource(
                loaded_assets.resource.clone(),
                transform,
                None,
                None,
                false,
                crate::commands::MaterialVariant::Base,
            );
        }
    }
}

fn add_animation(
    mut commands: Commands,
    query: Query<(Entity, &Name), (Without<AnimationPlayer>, With<SilkroadEntity>)>,
    assets: Res<AssetsToLoad>,
    skeleton_assets: Res<Assets<JMXVBSK>>,
    resource_assets: Res<Assets<SroResource>>,
    animation_assets: Res<Assets<JMXVBAN>>,
    mut animation_clips: ResMut<Assets<AnimationClip>>,
    mut animation_graphs: ResMut<Assets<AnimationGraph>>,
    input: Res<ButtonInput<KeyCode>>,
) {
    if input.just_released(KeyCode::Enter) {
        for (entity, name) in query.iter() {
            let mut anim_player = AnimationPlayer::default();
            let res = resource_assets.get(&assets.resource).expect("i failed");
            let skeleton = skeleton_assets
                .get(&res.skeleton.clone().unwrap())
                .expect("i failed");
            let animation = animation_assets.get(&assets.animation).expect("i failed");
            let animation_clip = animation.to_animation_clip(skeleton, &String::from(name));
            let animation_clip = animation_clips.add(animation_clip);
            let (anim_graph, anim_index) = AnimationGraph::from_clip(animation_clip);
            anim_player.play(anim_index).repeat();
            commands.entity(entity).insert((
                anim_player,
                AnimationGraphHandle(animation_graphs.add(anim_graph)),
            ));
        }
    }
}
