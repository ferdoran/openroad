use bevy::prelude::AlignSelf;

pub enum Alignment {
    Start,
    Center,
    End,
    Auto,
}

impl From<i32> for Alignment {
    fn from(value: i32) -> Self {
        match value {
            0 => Alignment::Start,
            1 => Alignment::Center,
            2 => Alignment::End,
            _ => Alignment::Auto,
        }
    }
}

impl From<Alignment> for AlignSelf {
    fn from(value: Alignment) -> Self {
        match value {
            Alignment::Start => AlignSelf::FlexStart,
            Alignment::Center => AlignSelf::Center,
            Alignment::End => AlignSelf::FlexEnd,
            Alignment::Auto => AlignSelf::Auto,
        }
    }
}
