use bevy::asset::Handle;
use bevy::prelude::Resource;

use crate::assets::char_select_scene::CharSelectScene;
use crate::assets::intro_scene::IntroScene;

#[derive(Resource)]
pub struct DesiredIntroSceneV2(pub Handle<IntroScene>);
#[derive(Resource)]
pub struct ActiveIntroSceneV2(pub IntroScene);

#[derive(Resource)]
pub struct DesiredCharSelectSceneV2(pub Handle<CharSelectScene>);
#[derive(Resource)]
pub struct ActiveCharSelectSceneV2(pub CharSelectScene);
