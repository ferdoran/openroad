use std::path::PathBuf;

use bevy::prelude::{Mat4, Vec3};
use bytes::Buf;

use crate::util::buf_ext::BufExt;

#[derive(Default)]
#[allow(dead_code)]
pub struct CollisionMesh {
    pub mesh_file: PathBuf,
    pub box_0: (Vec3, Vec3),
    pub box_1: (Vec3, Vec3),
    pub collision_matrix: Option<Mat4>,
}

impl<T: Buf + BufExt> From<&mut T> for CollisionMesh {
    fn from(value: &mut T) -> Self {
        Self {
            mesh_file: value.get_path_buf_double_len(),
            box_0: (value.get_vec3(), value.get_vec3()),
            box_1: (value.get_vec3(), value.get_vec3()),
            collision_matrix: if value.get_u32_le() == 1 {
                Some(value.get_mat4())
            } else {
                None
            },
        }
    }
}
