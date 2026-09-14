#[derive(Clone)]
pub enum EdgeDirection {
    Invalid = -1,
    North = 0,
    East = 1,
    South = 2,
    West = 3,
}

impl From<i8> for EdgeDirection {
    fn from(value: i8) -> Self {
        match value {
            0 => EdgeDirection::North,
            1 => EdgeDirection::East,
            2 => EdgeDirection::South,
            3 => EdgeDirection::West,
            _ => EdgeDirection::Invalid,
        }
    }
}
