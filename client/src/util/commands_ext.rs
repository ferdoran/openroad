use crate::assets::bsr::resource::SroResource;
use crate::commands::attach::{AttachResource, DetachResource};
use crate::commands::{MaterialVariant, SpawnResource};
use bevy::asset::Handle;
use bevy::prelude::{Commands, Entity, Transform};

pub trait CommandsExt {
    #[allow(clippy::too_many_arguments)]
    fn spawn_resource(
        &mut self,
        handle: Handle<SroResource>,
        transform: Transform,
        parent: Option<Entity>,
        animation_group: Option<String>,
        reverse_winding: bool,
        material_variant: MaterialVariant,
    );
    fn attach_resource(&mut self, item: Handle<SroResource>, character_wrapper: Entity);
    fn detach_resource(&mut self, item: Handle<SroResource>, character_wrapper: Entity);
}

impl CommandsExt for Commands<'_, '_> {
    fn spawn_resource(
        &mut self,
        handle: Handle<SroResource>,
        transform: Transform,
        parent: Option<Entity>,
        animation_group: Option<String>,
        reverse_winding: bool,
        material_variant: MaterialVariant,
    ) {
        self.queue(SpawnResource {
            resource: handle,
            transform,
            parent,
            animation_group,
            reverse_winding,
            material_variant,
        })
    }

    fn attach_resource(&mut self, item: Handle<SroResource>, character_wrapper: Entity) {
        self.queue(AttachResource {
            item,
            character_wrapper,
        })
    }

    fn detach_resource(&mut self, item: Handle<SroResource>, character_wrapper: Entity) {
        self.queue(DetachResource {
            item,
            character_wrapper,
        })
    }
}
