//! Idea: generates a small, exactly-tileable water ripple normal map from tileable-by-
//! construction value noise, rather than deriving one from any SRO game asset (there isn't a
//! real normal map in the original client's water textures to reuse). Each noise octave is
//! sampled on an integer-sized periodic lattice (`period` cells wrapped with `rem_euclid`), so
//! every octave tiles exactly at the UV wrap regardless of image resolution; summing several
//! octaves at doubling period / halving amplitude (a standard fBm) gives multi-scale ripple
//! detail without the visible repeating grid a small sum of pure sine waves produces. The
//! height field has no closed-form derivative (unlike sines), so the normal is recovered by
//! central-difference sampling the height a fraction of a texel to either side, exactly as a
//! shader would when it doesn't have an analytic slope available.
//!
//! Run via `cargo run -p tools --bin gen_water_normal` from the repo root whenever the ripple
//! look needs retuning; the PNG it writes is a generated asset checked into the repo like any
//! other, not something loaded from user-supplied game data.

use bevy::math::Vec3;
use image::{Rgb, RgbImage};

const SIZE: u32 = 512;
const OUT_PATH: &str = "assets/textures/water_normal.png";

/// fBm octave count and base period (in lattice cells); period doubles and amplitude halves
/// each octave. Base period must divide evenly into how the lattice wraps (any integer works,
/// `rem_euclid` handles the wrap) — picked small enough that the first octave reads as broad
/// swells, large enough by the last octave to read as fine ripple.
const OCTAVE_COUNT: i32 = 5;
const BASE_PERIOD: i32 = 4;

/// Overall slope-to-normal strength; higher values make the ripples read as steeper/choppier.
/// Small on purpose — a normal map should stay close to "flat" (pale blue, RGB near
/// (128,128,255)) with only gentle variation, not swing through the full color range.
const STRENGTH: f32 = 0.05;

/// Deterministic pseudo-random value in [-1, 1] for a lattice point, so re-running the
/// generator always reproduces the same texture.
fn hash(x: i32, y: i32, seed: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374761393)
        ^ (y as u32).wrapping_mul(668265263)
        ^ (seed as u32).wrapping_mul(2147483647);
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    h ^= h >> 16;
    (h as f32 / u32::MAX as f32) * 2.0 - 1.0
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Bilinearly-interpolated value noise on a `period`x`period` periodic lattice; tiles exactly
/// at u/v integer boundaries by construction.
fn value_noise_periodic(u: f32, v: f32, period: i32, seed: i32) -> f32 {
    let gx = u * period as f32;
    let gy = v * period as f32;
    let ix0 = gx.floor() as i32;
    let iy0 = gy.floor() as i32;
    let fx = gx - ix0 as f32;
    let fy = gy - iy0 as f32;

    let v00 = hash(ix0.rem_euclid(period), iy0.rem_euclid(period), seed);
    let v10 = hash((ix0 + 1).rem_euclid(period), iy0.rem_euclid(period), seed);
    let v01 = hash(ix0.rem_euclid(period), (iy0 + 1).rem_euclid(period), seed);
    let v11 = hash(
        (ix0 + 1).rem_euclid(period),
        (iy0 + 1).rem_euclid(period),
        seed,
    );

    let sx = smoothstep(fx);
    let sy = smoothstep(fy);
    lerp(lerp(v00, v10, sx), lerp(v01, v11, sx), sy)
}

fn fbm_height(u: f32, v: f32) -> f32 {
    let mut height = 0.0;
    let mut amplitude = 1.0;
    let mut amplitude_sum = 0.0;
    let mut period = BASE_PERIOD;
    for octave in 0..OCTAVE_COUNT {
        height += value_noise_periodic(u, v, period, 1000 + octave) * amplitude;
        amplitude_sum += amplitude;
        amplitude *= 0.5;
        period *= 2;
    }
    height / amplitude_sum
}

fn main() {
    std::fs::create_dir_all("assets/textures").expect("failed to create assets/textures");

    let mut img = RgbImage::new(SIZE, SIZE);
    let eps = 0.5 / SIZE as f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let u = x as f32 / SIZE as f32;
            let v = y as f32 / SIZE as f32;

            let d_du = (fbm_height(u + eps, v) - fbm_height(u - eps, v)) / (2.0 * eps);
            let d_dv = (fbm_height(u, v + eps) - fbm_height(u, v - eps)) / (2.0 * eps);

            let n = Vec3::new(-d_du * STRENGTH, -d_dv * STRENGTH, 1.0).normalize();
            let r = ((n.x * 0.5 + 0.5) * 255.0).round() as u8;
            let g = ((n.y * 0.5 + 0.5) * 255.0).round() as u8;
            let b = ((n.z * 0.5 + 0.5) * 255.0).round() as u8;
            img.put_pixel(x, y, Rgb([r, g, b]));
        }
    }

    img.save(OUT_PATH)
        .expect("failed to write water normal map");
    println!("wrote {OUT_PATH}");
}
