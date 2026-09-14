#[derive(Clone)]
#[allow(dead_code)]
pub struct LinkEdge {
    pub(crate) linked_obj_id: i16,
    pub(crate) linked_obj_edge_id: i16,
    pub(crate) edge_id: i16,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct LinkEdges(pub Vec<LinkEdge>);
