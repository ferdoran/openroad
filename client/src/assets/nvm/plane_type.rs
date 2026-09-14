#[derive(Clone)]
pub enum PlaneType {
    None = 0,
    Water = 1,
    Ice = 2,
    // Bit 0 (Water) and bit 1 (Ice) can both be set, e.g. frozen water.
    WaterIce = 3,
}

impl PlaneType {
    /// Whether this plane carries an ice sheet (`Ice` or `WaterIce`) — a solid
    /// surface an actor stands *on*, as opposed to plain `Water` (unwalkable) or
    /// `None`. Bit 1 is the ice bit.
    pub fn is_ice(&self) -> bool {
        matches!(self, PlaneType::Ice | PlaneType::WaterIce)
    }
}

impl From<u8> for PlaneType {
    fn from(value: u8) -> Self {
        match value {
            0 => PlaneType::None,
            1 => PlaneType::Water,
            2 => PlaneType::Ice,
            3 => PlaneType::WaterIce,
            x => {
                println!("Invalid byte value '{x}' for PlaneType");
                PlaneType::None
            }
        }
    }
}
