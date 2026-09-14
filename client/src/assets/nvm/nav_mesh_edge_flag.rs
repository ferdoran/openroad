/// Flag byte carried by every nav mesh edge — both the terrain edges of a
/// `.nvm` region (global/internal lists) and the outline/inline edges of an
/// object's `.bms` nav mesh.
///
/// The two block bits are *directional*, relative to the edge's own `src`→`dst`
/// orientation: which of them applies depends on which side of the edge the
/// mover is coming from. Callers that can't establish that side (no associated
/// cell to fix the convention) fall back to [`is_blocked`](Self::is_blocked).
///
/// ```text
/// None         = 0
/// BlockDst2Src = 1
/// BlockSrc2Dst = 2
/// Blocked      = 3    // BlockDst2Src | BlockSrc2Dst
/// Internal     = 4    // edge inside one nav mesh
/// Global       = 8    // edge on a nav mesh boundary; on an object outline
///                     // edge this marks a transfer point to terrain/another
///                     // object (a hint only — unmarked outlines still let a
///                     // mover walk off, or stairs would trap them)
/// Underpass    = 16   // actor passthrough from outside, blocked from inside
/// Entrance     = 32   // dungeon (obsolete?)
/// Bit6         = 64
/// Siege        = 128  // fortress war: attack passthrough
/// ```
///
/// Source: https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/EdgeFlag
#[derive(Clone)]
pub struct NavMeshEdgeFlag(pub u8);

impl NavMeshEdgeFlag {
    pub fn is_none(&self) -> bool {
        self.0 == 0
    }

    /// Blocked when moving from the `dst` side to the `src` side.
    pub fn is_blocked_dst_to_src(&self) -> bool {
        self.0 & (1 << 0) != 0
    }

    /// Blocked when moving from the `src` side to the `dst` side.
    pub fn is_blocked_src_to_dst(&self) -> bool {
        self.0 & (1 << 1) != 0
    }

    /// Blocked in *either* direction — the non-directional test, used when the
    /// mover's side of the edge can't be established.
    pub fn is_blocked(&self) -> bool {
        self.0 & ((1 << 0) | (1 << 1)) != 0
    }

    /// Blocked in both directions.
    pub fn is_fully_blocked(&self) -> bool {
        self.0 & ((1 << 0) | (1 << 1)) == ((1 << 0) | (1 << 1))
    }

    pub fn is_internal(&self) -> bool {
        self.0 & (1 << 2) != 0
    }

    pub fn is_global(&self) -> bool {
        self.0 & (1 << 3) != 0
    }

    /// Passable from outside the object, blocked from inside it.
    pub fn is_underpass(&self) -> bool {
        self.0 & (1 << 4) != 0
    }

    pub fn is_entrance(&self) -> bool {
        self.0 & (1 << 5) != 0
    }

    pub fn is_bit6(&self) -> bool {
        self.0 & (1 << 6) != 0
    }

    pub fn is_siege(&self) -> bool {
        self.0 & (1 << 7) != 0
    }
}
