//! Region-id decoding. A 16-bit region id packs the overworld sector as
//! X = bits 0-7, Z = bits 8-14 — and bit 15 is the *dungeon flag*, not part
//! of Z (go-sro/RSBot `RID` convention; dungeon ids therefore read as
//! negative i16, cf. `zonenames.rs`). For a dungeon id the low byte is the
//! `dungeoninfo.txt` dungeon index and there is no sector grid at all, so
//! overworld math (tiling, minimap, environment) must gate on `is_dungeon()`
//! before consuming `to_x_z()`.

use bevy::prelude::Vec3;

pub trait RegionIdExt {
    fn to_x_z(&self) -> (u8, u8);
    /// Bit 15 set = dungeon region (coordinates are dungeon-local, no
    /// 1920-unit sector tiling).
    fn is_dungeon(&self) -> bool;
}

macro_rules! impl_region_id_ext {
    ($ty:ty) => {
        impl RegionIdExt for $ty {
            fn to_x_z(&self) -> (u8, u8) {
                let id = *self as u16;
                let x: u8 = (id & 0xFF) as u8;
                // 7 bits — bit 15 is the dungeon flag, never part of Z.
                let z: u8 = (id >> 8 & 0x7F) as u8;
                (x, z)
            }

            fn is_dungeon(&self) -> bool {
                (*self as u16) & 0x8000 != 0
            }
        }
    };
}

impl_region_id_ext!(u16);
impl_region_id_ext!(i16);

impl RegionIdExt for Vec3 {
    fn to_x_z(&self) -> (u8, u8) {
        if self.x.is_sign_negative() {
            return (
                (-self.x as f32 / 1920.0) as u8,
                (self.z as f32 / 1920.0) as u8,
            );
        }
        (
            (self.x as f32 / 1920.0) as u8,
            (self.z as f32 / 1920.0) as u8,
        )
    }

    /// An SRO-space position is always overworld space; dungeon interiors
    /// live in their own local frame and never reach this conversion.
    fn is_dungeon(&self) -> bool {
        false
    }
}

#[allow(dead_code)]
pub trait RegionExt {
    fn region_offset(&self) -> Vec3;
}

impl RegionExt for Vec3 {
    fn region_offset(&self) -> Vec3 {
        Vec3::new(self.x % 1920.0, 0.0, self.z % 1920.0)
    }
}

#[cfg(test)]
mod test {
    use bevy::prelude::Vec3;
    use rand::Rng;

    use crate::util::region::RegionIdExt;

    #[test]
    fn region_id_to_x_z() {
        // Jangan: 25000 = 0x61A8 = sector 168x97.
        assert_eq!(25000u16.to_x_z(), (168, 97));
        assert!(!25000u16.is_dungeon());
        // Donwhang cave: 0x8001 (= -32767 as i16). The dungeon flag must not
        // fold into Z (the old decode returned z = 128 here).
        assert_eq!(0x8001u16.to_x_z(), (1, 0));
        assert!(0x8001u16.is_dungeon());
        assert_eq!((-32767i16).to_x_z(), (1, 0));
        assert!((-32767i16).is_dungeon());
        // jinsi_floor02 via gm_event collision id: 0x8006.
        assert_eq!(0x8006u16.to_x_z(), (6, 0));
    }

    #[test]
    fn vec3_to_x_z() {
        let x: u8 = rand::rng().random_range(0..255);
        let z: u8 = rand::rng().random_range(0..128);

        let v = Vec3::new(x as f32 * 1920.0, 0.0, z as f32 * 1920.0);

        let (actual_x, actual_z) = v.to_x_z();
        assert_eq!(actual_x, x);
        assert_eq!(actual_z, z);
    }

    #[test]
    fn vec3_to_x_z_with_fraction() {
        let x: u8 = rand::rng().random_range(0..255);
        let z: u8 = rand::rng().random_range(0..128);

        let mut v = Vec3::new(x as f32 * 1920.0, 0.0, z as f32 * 1920.0);
        v.x += rand::rng().random_range(0.0..1920.0);
        v.z += rand::rng().random_range(0.0..1920.0);

        let (actual_x, actual_z) = v.to_x_z();
        assert_eq!(actual_x, x);
        assert_eq!(actual_z, z);
    }
}
