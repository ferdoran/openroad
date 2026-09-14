use std::borrow::BorrowMut;
use std::io::Cursor;

use bevy::prelude::*;
use bevy::reflect::TypePath;
use bytes::Buf;

use crate::assets::read_str_and_jump;

#[derive(Debug)]
#[allow(dead_code)]
pub struct ColorGraph {
    pub key_count: i32,
    pub keys: Vec<Vec4>,
}

impl From<&mut Cursor<&[u8]>> for ColorGraph {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        let key_count = cursor.get_i32_le();
        let mut keys: Vec<Vec4> = Vec::with_capacity(key_count as usize);
        for _ in 0..key_count {
            let r = cursor.get_f32_le(); // red
            let g = cursor.get_f32_le(); // green
            let b = cursor.get_f32_le(); // blue
            let w = cursor.get_f32_le(); // time
            keys.push(Vec4::new(r, g, b, w));
        }
        ColorGraph { key_count, keys }
    }
}

impl ColorGraph {
    /// Sample the RGB color at day-cycle time `t` (0.0..1.0, wraps). `None` if empty.
    pub fn sample(&self, t: f32) -> Option<Vec3> {
        sample_graph(
            &self.keys,
            t,
            |k| k.w,
            |a, b, s| a.truncate().lerp(b.truncate(), s),
        )
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct FloatGraph {
    pub key_count: i32,
    pub keys: Vec<Vec2>,
}

impl From<&mut Cursor<&[u8]>> for FloatGraph {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        let key_count = cursor.get_i32_le();
        let mut keys: Vec<Vec2> = Vec::with_capacity(key_count as usize);
        for _ in 0..key_count {
            let value = cursor.get_f32_le(); // value
            let time = cursor.get_f32_le(); // time
            keys.push(Vec2::new(value, time));
        }
        FloatGraph { key_count, keys }
    }
}

impl FloatGraph {
    /// Sample the value at day-cycle time `t` (0.0..1.0, wraps). `None` if empty.
    pub fn sample(&self, t: f32) -> Option<f32> {
        sample_graph(&self.keys, t, |k| k.y, |a, b, s| a.x + (b.x - a.x) * s)
    }
}

/// Shared interpolation core for `ColorGraph`/`FloatGraph`: linear lerp between the two
/// keys straddling `t`, treating the graph as cyclic over one day — before the first or
/// after the last key, it blends across the midnight boundary (last key → first key).
/// Keys are assumed sorted by time, as they are in the JMXVENVI data.
fn sample_graph<K, V>(
    keys: &[K],
    t: f32,
    time_of: impl Fn(&K) -> f32,
    lerp: impl Fn(&K, &K, f32) -> V,
) -> Option<V> {
    let first = keys.first()?;
    let last = keys.last()?;
    if keys.len() == 1 {
        return Some(lerp(first, first, 0.0));
    }
    let t = t.rem_euclid(1.0);
    let (t_first, t_last) = (time_of(first), time_of(last));
    if t < t_first || t >= t_last {
        let span = 1.0 - t_last + t_first;
        let elapsed = if t >= t_last {
            t - t_last
        } else {
            t + 1.0 - t_last
        };
        let s = if span <= f32::EPSILON {
            0.0
        } else {
            elapsed / span
        };
        return Some(lerp(last, first, s));
    }
    let next = keys.iter().position(|k| time_of(k) > t)?;
    let (a, b) = (&keys[next - 1], &keys[next]);
    let span = time_of(b) - time_of(a);
    let s = if span <= f32::EPSILON {
        0.0
    } else {
        (t - time_of(a)) / span
    };
    Some(lerp(a, b, s))
}

#[allow(dead_code)]
pub struct Environment {
    pub child_count: u32,
    pub name_length: u32,
    pub name: String,

    /// Index into the profile list. Reading this as an `i32` swallowed the
    /// following `Short0` into the high word: `Short0` is the node's tree
    /// depth in all 78 corpus nodes, so every depth-2 leaf decoded as
    /// `real_id | 0x20000` (131,072… instead of 0…59) and every depth-1 node
    /// as `real_id | 0x10000`.
    pub profile_id: u16,
    /// Tree depth. Editor grouping only — see [`Environment`].
    pub short0: u16,
    /// Also the tree depth.
    pub int0: i32,
    /// Depth for 68 nodes, but depth+1 for exactly 10 named leaves (Jangan,
    /// Donwhang village, …). UNKNOWN (docs/re/formats/envi-jmxvenvi.md §9).
    pub int1: i32,

    pub children: Vec<Environment>,
}

impl From<&mut Cursor<&[u8]>> for Environment {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        // Little-endian throughout: `_ne` was correct only because every
        // supported target is little-endian.
        let child_count = cursor.get_u32_le();
        let name_length = cursor.get_u32_le();
        let name = read_str_and_jump(cursor, name_length as u64);

        let profile_id = cursor.get_u16_le();
        let short0 = cursor.get_u16_le();

        let int0 = cursor.get_i32_le();
        let int1 = cursor.get_i32_le();

        let mut children = Vec::with_capacity(child_count as usize);

        for _ in 0..child_count {
            let child = Environment::from(cursor.borrow_mut());
            children.push(child);
        }

        Environment {
            child_count,
            name_length,
            name,
            profile_id,
            short0,
            int0,
            int1,
            children,
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct EnvironmentProfile {
    pub id: u16,
    pub name_length: u32,
    pub name: String,
    pub string0_length: u32,
    pub string0: String,
    pub string1_length: u32,
    pub string1: String,

    pub sun_color: ColorGraph,
    pub sky_top_color: ColorGraph,
    pub diffuse_color: ColorGraph,
    pub object_ambient_color: ColorGraph,
    /// Inferred (no official name — SilkroadDoc calls it Graph4): white at noon, warm
    /// at dawn/dusk, dark navy at night, near-black in dungeon profiles — the tint for
    /// the scrolling cloud layers. See docs/formats/envi-jmxvenvi.md.
    pub cloud_color: ColorGraph,
    pub terrain_ambient_color: ColorGraph,
    pub terrain_shadow_color: ColorGraph, //added in fileVersion 1003
    /// Like all FloatGraphs in this format: normalized to [-1, 1], mapped onto a real
    /// range by the client. Both planes push outward toward noon and pull in at night
    /// (SRO's short night view distance).
    pub fog_near_plane: FloatGraph,
    pub fog_far_plane: FloatGraph,
    pub fog_color: ColorGraph,
    /// Inferred (Graph10/Graph11): per-layer cloud opacities — Graph10 varies with
    /// daylight around its authoring default of 0.5 (the small cloud1.ddj layer),
    /// Graph11 hovers near 1.0 with default 0.9 (the large cloud99.ddj layer).
    pub cloud_near_alpha: FloatGraph,
    pub cloud_far_alpha: FloatGraph,
    /// Unknown purpose: oscillates between -1 and 1 with many keys and no consistent
    /// day-cycle correlation (gentle in towns, wild in some wilderness profiles) —
    /// possibly wind / cloud-scroll modulation.
    pub graph12: FloatGraph, //added in fileVersion 1001
    pub sky_bottom_color: ColorGraph, //added in fileVersion 1002
    pub water_color: ColorGraph,      //added in fileVersion 1002
    /// Inferred (Graph15): +1 at midnight, -1 at noon in every outdoor profile — a
    /// "nightness" factor. Drives the procedural star field's intensity: remapped to 0..1 in
    /// `plugins/environment` and written to `gradient.params.x`, which `sky_gradient.wgsl`
    /// feeds to `stars()`.
    pub night_intensity: FloatGraph,
}

impl From<&mut Cursor<&[u8]>> for EnvironmentProfile {
    fn from(cursor: &mut Cursor<&[u8]>) -> Self {
        //println!("Position {:?}", cursor.position());
        let id = cursor.get_u16_le();
        //println!("ID {:?}", id);
        let name_length = cursor.get_u32_le();
        //println!("Name Length {:?}", name_length);
        let name = read_str_and_jump(cursor.borrow_mut(), name_length as u64);
        //println!("Name {:?}", name);

        let string0_length = cursor.get_u32_le();
        let string0 = read_str_and_jump(cursor.borrow_mut(), string0_length as u64);
        let string1_length = cursor.get_u32_le();
        let string1 = read_str_and_jump(cursor.borrow_mut(), string1_length as u64);
        let sun_color = ColorGraph::from(cursor.borrow_mut());
        let sky_top_color = ColorGraph::from(cursor.borrow_mut());
        let diffuse_color = ColorGraph::from(cursor.borrow_mut());
        let object_ambient_color = ColorGraph::from(cursor.borrow_mut());
        let cloud_color = ColorGraph::from(cursor.borrow_mut());
        let terrain_ambient_color = ColorGraph::from(cursor.borrow_mut());
        let terrain_shadow_color = ColorGraph::from(cursor.borrow_mut());
        let fog_near_plane = FloatGraph::from(cursor.borrow_mut());
        let fog_far_plane = FloatGraph::from(cursor.borrow_mut());
        let fog_color = ColorGraph::from(cursor.borrow_mut());
        let cloud_near_alpha = FloatGraph::from(cursor.borrow_mut());
        let cloud_far_alpha = FloatGraph::from(cursor.borrow_mut());
        let graph12 = FloatGraph::from(cursor.borrow_mut());
        let sky_bottom_color = ColorGraph::from(cursor.borrow_mut());
        let water_color = ColorGraph::from(cursor.borrow_mut());
        let night_intensity = FloatGraph::from(cursor.borrow_mut());

        EnvironmentProfile {
            id,
            name_length,
            name,
            string0_length,
            string0,
            string1_length,
            string1,
            sun_color,
            sky_top_color,
            diffuse_color,
            object_ambient_color,
            cloud_color,
            terrain_ambient_color,
            terrain_shadow_color,
            fog_near_plane,
            fog_far_plane,
            fog_color,
            cloud_near_alpha,
            cloud_far_alpha,
            graph12,
            sky_bottom_color,
            water_color,
            night_intensity,
        }
    }
}

#[derive(TypePath, Asset)]
#[allow(dead_code)]
pub struct JMXVENVI {
    pub format: String,
    pub version: String,
    pub profile_count: i16,
    pub environment_set_name_length: u32,
    pub environment_set_name: String,
    pub profiles: Vec<EnvironmentProfile>,
    pub environment_root: Environment,
}

impl From<&[u8]> for JMXVENVI {
    fn from(bytes: &[u8]) -> Self {
        //println!("Bytes Length: {:?}", bytes.len());
        let mut cursor = Cursor::new(bytes);
        let format = read_str_and_jump(&mut cursor, 8);
        let version = read_str_and_jump(&mut cursor, 4);
        //println!("Signture: {:?}", signature);
        let profile_count = cursor.get_i16_le();
        //println!("Profiles: {:?}", profile_count);
        let environment_set_name_length = cursor.get_u32_le();
        //println!("Env Name Length: {:?}", environment_set_name_length);
        let environment_set_name =
            read_str_and_jump(&mut cursor, environment_set_name_length as u64);
        //println!("Env Set Name: {:?}", environment_set_name);
        let mut profiles: Vec<EnvironmentProfile> = Vec::with_capacity(profile_count as usize);

        for _ in 0..profile_count {
            //println!("Profile {:?} --->", i);
            let profile = EnvironmentProfile::from(&mut cursor);
            profiles.push(profile);
        }

        let environment_root = Environment::from(&mut cursor);

        JMXVENVI {
            format,
            version,
            profile_count: profile_count as i16,
            environment_set_name_length,
            environment_set_name,
            profiles,
            environment_root,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color_graph(keys: &[(f32, f32, f32, f32)]) -> ColorGraph {
        ColorGraph {
            key_count: keys.len() as i32,
            keys: keys
                .iter()
                .map(|(r, g, b, t)| Vec4::new(*r, *g, *b, *t))
                .collect(),
        }
    }

    fn float_graph(keys: &[(f32, f32)]) -> FloatGraph {
        FloatGraph {
            key_count: keys.len() as i32,
            keys: keys.iter().map(|(v, t)| Vec2::new(*v, *t)).collect(),
        }
    }

    #[test]
    fn empty_graph_samples_none() {
        assert_eq!(color_graph(&[]).sample(0.5), None);
        assert_eq!(float_graph(&[]).sample(0.5), None);
    }

    #[test]
    fn single_key_is_constant() {
        let graph = color_graph(&[(0.3, 0.6, 0.9, 0.4)]);
        assert_eq!(graph.sample(0.0), Some(Vec3::new(0.3, 0.6, 0.9)));
        assert_eq!(graph.sample(0.9), Some(Vec3::new(0.3, 0.6, 0.9)));
    }

    fn assert_close(actual: Option<f32>, expected: f32) {
        let actual = actual.expect("expected a sample");
        assert!(
            (actual - expected).abs() < 1e-4,
            "expected ~{expected}, got {actual}"
        );
    }

    #[test]
    fn midpoint_lerps_between_keys() {
        let graph = float_graph(&[(0.0, 0.2), (1.0, 0.6)]);
        assert_close(graph.sample(0.4), 0.5);
    }

    #[test]
    fn exact_key_time_returns_key_value() {
        let graph = float_graph(&[(1.0, 0.2), (3.0, 0.5), (5.0, 0.8)]);
        assert_eq!(graph.sample(0.2), Some(1.0));
        assert_eq!(graph.sample(0.5), Some(3.0));
        assert_eq!(graph.sample(0.8), Some(5.0));
    }

    #[test]
    fn wraps_across_midnight() {
        // Keys at 0.9 and 0.1: midnight (t=0.0) lies exactly halfway through the
        // wrap-around segment.
        let graph = float_graph(&[(10.0, 0.1), (30.0, 0.9)]);
        assert_close(graph.sample(0.0), 20.0);
        // Quarter into the wrap segment (t=0.95) and just before the first key.
        assert_close(graph.sample(0.95), 25.0);
        assert_close(graph.sample(0.05), 15.0);
    }

    #[test]
    fn time_outside_unit_range_wraps() {
        let graph = float_graph(&[(0.0, 0.2), (1.0, 0.6)]);
        assert_close(graph.sample(1.4), 0.5);
        assert_close(graph.sample(-0.6), 0.5);
    }

    /// One `Environment` node: child_count, name_length, name, profile_id,
    /// short0, int0, int1.
    fn env_node(name: &str, profile_id: u16, depth: u16) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0u32.to_le_bytes()); // no children
        b.extend_from_slice(&(name.len() as u32).to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&profile_id.to_le_bytes());
        b.extend_from_slice(&depth.to_le_bytes()); // Short0 == tree depth
        b.extend_from_slice(&(depth as i32).to_le_bytes()); // Int0
        b.extend_from_slice(&(depth as i32).to_le_bytes()); // Int1
        b
    }

    /// Reading ProfileId as an i32 swallowed Short0 into the high word. Since
    /// Short0 is the node's depth, a depth-2 leaf's id came out as
    /// `real_id | 0x20000` — 131,072+ instead of 0..59 (#292).
    #[test]
    fn profile_id_does_not_swallow_short0() {
        let bytes = env_node("Jangan", 42, 2);
        let mut cursor = Cursor::new(bytes.as_slice());
        let env = Environment::from(&mut cursor);
        assert_eq!(env.name, "Jangan");
        assert_eq!(env.profile_id, 42, "must not be 42 | 0x20000 = 131114");
        assert_eq!(env.short0, 2, "Short0 is the tree depth");
        assert_eq!(env.int0, 2);
    }

    /// Every shipped profile id is in 0..=59, so none may decode above that.
    #[test]
    fn every_corpus_profile_id_stays_in_range() {
        for (id, depth) in [(0u16, 0u16), (59, 1), (17, 2)] {
            let bytes = env_node("n", id, depth);
            let mut cursor = Cursor::new(bytes.as_slice());
            let env = Environment::from(&mut cursor);
            assert_eq!(env.profile_id, id);
            assert!(env.profile_id <= 59, "corpus ids are 0..=59");
        }
    }
}
