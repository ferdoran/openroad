use bevy::animation::graph::AnimationGraphHandle;
use bevy::app::App;
use bevy::asset::Assets;
use bevy::prelude::*;
use bevy_asset_loader::prelude::{
    AssetCollection, ConfigureLoadingState, LoadingStateAppExt, LoadingStateConfig,
};

use crate::plugins::camera::DebugCamera;

use crate::assets::ban::JMXVBAN;
use crate::assets::bsk::JMXVBSK;
use crate::assets::bsr::resource::SroResource;
use crate::plugins::camera::spawn_player_camera;
use crate::plugins::dynamic_resource_loader::MirroredResource;
use crate::plugins::map::objects::*;
use crate::scenes::SceneState;
use crate::util::mesh::needs_winding_reversal;
use crate::GameState;

pub struct AnimationTestingScenePlugin;

impl Plugin for AnimationTestingScenePlugin {
    fn build(&self, app: &mut App) {
        // app.add_loading_state(
        //     LoadingState::new(SceneState::Loading)
        //     .continue_to_state(SceneState::AnimationTesting)
        // )
        app
            .configure_loading_state(
                LoadingStateConfig::new(SceneState::Loading)
                    .load_collection::<AnimationAssets>()
            )
            // .add_plugins(ProgressPlugin::new(SceneState::Loading).continue_to(SceneState::AnimationTesting))
        ;

        app.add_systems(
            OnEnter(GameState::Game),
            (
                spawn_player_camera.run_if(in_state(SceneState::AnimationTesting)),
                spawn_mesh.run_if(in_state(SceneState::AnimationTesting)),
            )
                .chain(),
        )
        .add_systems(
            Update,
            add_animation.run_if(in_state(SceneState::AnimationTesting)),
        );
    }
}

#[derive(AssetCollection, Resource)]
struct AnimationAssets {
    #[asset(path = "data://prim/ani/mob/oasis/blackrobber/blackrobber_attack02.ban")]
    // #[asset(path = "data://prim/ani/npc/china/chinaetc_kisaeng6_basic.ban")]
    // #[asset(path = "data://prim/ani/char/china/woman/chinawoman_standbattle.ban")]
    // #[asset(path = "data://prim/ani/mob/roc/roc_stand01.ban")]
    pub anim: Handle<JMXVBAN>,
    // #[asset(path = "data://prim/mesh/npc/china/blackrobber_body_part1.bms")]
    // pub black_robber_1: Handle<JMXVBMS>,
    // #[asset(path = "data://prim/mesh/npc/china/blackrobber_body_part2.bms")]
    // pub black_robber_2: Handle<JMXVBMS>,
    // #[asset(path = "data://prim/skel/npc/china/blackrobber.bsk")]
    // pub black_robber_skeleton: Handle<JMXVBAN>,
    #[asset(path = "data://res/mob/oasis/blackrobber.bsr")]
    // #[asset(path = "data://res/mob/china/mangnyang.bsr")]
    // #[asset(path = "data://res/npc/npc/chinaetc_kisaeng6.bsr")]
    // #[asset(path = "data://res/char/china/chinawoman_kisaeng.bsr")]
    // #[asset(path = "data://res/mob/roc/roc.bsr")]
    pub black_robber: Handle<SroResource>,

    #[asset(path = "data://prim/skel/npc/china/blackrobber.bsk")]
    // #[asset(path = "data://prim/skel/npc/china/chinaetc_kisaeng6.bsk")]
    // #[asset(path = "data://prim/skel/char/china/chinawoman_skel.bsk")]
    // #[asset(path = "data://prim/skel/mob/roc/roc.bsk")]
    pub black_robber_skeleton: Handle<JMXVBSK>,
}

fn spawn_mesh(
    mut commands: Commands,
    assets: Res<AnimationAssets>,
    mut query: Query<&mut Transform, With<DebugCamera>>,
) {
    info!("spawning anim mesh");
    commands.init_resource::<SroMeshes>();
    commands.init_resource::<SroBindPoses>();
    commands.insert_resource(GlobalAmbientLight {
        brightness: 100.0,
        color: Color::WHITE,
        ..default()
    });

    // Mirrored on X like every SRO resource; the shared winding rule reverses
    // its meshes because the placement determinant is negative.
    let transform = Transform::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    let mut e = commands.spawn((
        transform,
        Visibility::default(),
        LoadingResources(vec![assets.black_robber.clone()]),
    ));
    if needs_winding_reversal(&transform.to_matrix()) {
        e.insert(MirroredResource);
    }

    let Ok(mut trans) = query.single_mut() else {
        return;
    };
    trans.translation = Vec3::ZERO;
}

/// Marks wrapper entities whose player was already retargeted to [`AnimationAssets::anim`].
#[derive(Component)]
struct TestAnimationApplied;

/// [`SpawnResource`](crate::commands::SpawnResource) auto-plays the first animation of the
/// BSR on the wrapper entity. Retarget that player to the clip selected in
/// [`AnimationAssets`]. The wrapper's `Name` is the resource name, which is also the root
/// of the bone target-id paths, so the clip must be built with it.
fn add_animation(
    mut commands: Commands,
    mut query: Query<
        (
            Entity,
            &Name,
            &mut AnimationPlayer,
            &mut AnimationGraphHandle,
        ),
        Without<TestAnimationApplied>,
    >,
    assets: Res<AnimationAssets>,
    skeleton_assets: Res<Assets<JMXVBSK>>,
    animation_assets: Res<Assets<JMXVBAN>>,
    mut animation_clips: ResMut<Assets<AnimationClip>>,
    mut animation_graphs: ResMut<Assets<AnimationGraph>>,
) {
    for (entity, name, mut anim_player, mut graph_handle) in query.iter_mut() {
        let Some(skeleton) = skeleton_assets.get(&assets.black_robber_skeleton) else {
            continue;
        };
        let Some(animation) = animation_assets.get(&assets.anim) else {
            continue;
        };
        let animation_clip = animation.to_animation_clip(skeleton, &String::from(name));
        let animation_clip = animation_clips.add(animation_clip);
        let (anim_graph, anim_index) = AnimationGraph::from_clip(animation_clip);
        anim_player.stop_all();
        anim_player.play(anim_index).repeat();
        *graph_handle = AnimationGraphHandle(animation_graphs.add(anim_graph));
        commands.entity(entity).insert(TestAnimationApplied);
    }
}
