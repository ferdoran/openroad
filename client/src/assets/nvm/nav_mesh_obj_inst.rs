use crate::assets::nvm::link_edge::LinkEdges;
use crate::assets::nvm::map_object::MapObject;

#[derive(Clone)]
#[allow(dead_code)]
pub struct NavMeshObjInst {
    pub(crate) object: MapObject,
    pub(crate) link_edges: LinkEdges,
}
