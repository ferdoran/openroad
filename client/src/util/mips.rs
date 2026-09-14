//! Runtime mip-chain generation for tightly-packed RGBA8 pixel data.
//!
//! Some SRO texture paths lack authored mipmaps (single-level hand-decoded
//! `.ddj` sources; the water normal map is a
//! PNG, which Bevy never generates mips for), so distant geometry samples
//! full-resolution textures — aliasing plus texture-cache thrashing. These
//! helpers rebuild a full chain with an area box filter, in exactly the layout
//! wgpu's `create_texture_with_data` expects: all levels tightly packed one
//! after another (mip 0 first), no row padding — Bevy uploads per mip. Level
//! dimensions are floor(size >> level).max(1), matching wgpu's
//! `mip_level_size`.

/// Number of mip levels for a `width x height` base image (full chain down
/// to 1x1).
fn level_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

fn level_size(base: u32, level: u32) -> u32 {
    (base >> level).max(1)
}

/// Builds the full mip chain for RGBA8 `mip0`. Returns
/// `(mip_level_count, concatenated level data)`. Non-colour values stay linear.
/// Use the sRGB variant for colour textures.
pub fn rgba8_mip_chain(width: u32, height: u32, mip0: &[u8]) -> (u32, Vec<u8>) {
    rgba8_mip_chain_with(width, height, mip0, Filter::Linear)
}

/// Like [`rgba8_mip_chain`], but for tangent-space normal maps: each output
/// texel decodes its source footprint to [-1, 1] vectors, averages, and
/// renormalizes — a plain box filter progressively flattens normals toward
/// (0, 0, 0.5-ish) instead. Alpha is box-filtered.
pub fn rgba8_normal_mip_chain(width: u32, height: u32, mip0: &[u8]) -> (u32, Vec<u8>) {
    rgba8_mip_chain_with(width, height, mip0, Filter::Normal)
}

/// Filter colour in linear light with alpha-weighted RGB to avoid dark fringes.
/// Alpha remains area-averaged: no material-specific mask threshold is guessed.
pub fn rgba8_srgb_mip_chain(width: u32, height: u32, mip0: &[u8]) -> (u32, Vec<u8>) {
    rgba8_mip_chain_with(width, height, mip0, Filter::Srgb)
}

#[derive(Clone, Copy)]
enum Filter {
    Linear,
    Srgb,
    Normal,
}

// Area overlap gives odd-sized edge pixels their full share, including 1D tails.
// Accumulate decoded values, then encode once per destination texel.
fn rgba8_mip_chain_with(width: u32, height: u32, mip0: &[u8], filter: Filter) -> (u32, Vec<u8>) {
    debug_assert_eq!(mip0.len(), (width * height * 4) as usize);
    let levels = level_count(width, height);
    if levels <= 1 {
        return (1, mip0.to_vec());
    }

    // total size: sum over levels of w_i * h_i * 4
    let total: usize = (0..levels)
        .map(|l| (level_size(width, l) * level_size(height, l) * 4) as usize)
        .sum();
    let mut data = Vec::with_capacity(total);
    data.extend_from_slice(mip0);

    let mut src_offset = 0usize;
    for level in 1..levels {
        let sw = level_size(width, level - 1) as usize;
        let sh = level_size(height, level - 1) as usize;
        let dw = level_size(width, level) as usize;
        let dh = level_size(height, level) as usize;

        let dst_offset = data.len();
        for y in 0..dh {
            for x in 0..dw {
                let (x0, x1) = (
                    x as f32 * sw as f32 / dw as f32,
                    (x + 1) as f32 * sw as f32 / dw as f32,
                );
                let (y0, y1) = (
                    y as f32 * sh as f32 / dh as f32,
                    (y + 1) as f32 * sh as f32 / dh as f32,
                );
                let mut sum = [0.0; 4];
                let mut weight = 0.0;
                for sy in y0.floor() as usize..(y1.ceil() as usize).min(sh) {
                    for sx in x0.floor() as usize..(x1.ceil() as usize).min(sw) {
                        let area = ((sx + 1) as f32).min(x1) - (sx as f32).max(x0);
                        let area = area * (((sy + 1) as f32).min(y1) - (sy as f32).max(y0));
                        let o = src_offset + (sy * sw + sx) * 4;
                        let alpha = data[o + 3] as f32 / 255.0;
                        for c in 0..3 {
                            let v = data[o + c] as f32 / 255.0;
                            sum[c] += area
                                * match filter {
                                    Filter::Linear => v,
                                    Filter::Srgb => bevy::color::Srgba::gamma_function(v) * alpha,
                                    Filter::Normal => v * 2.0 - 1.0,
                                };
                        }
                        sum[3] += area * alpha;
                        weight += area;
                    }
                }
                let alpha = sum[3] / weight;
                let mut rgb = [sum[0] / weight, sum[1] / weight, sum[2] / weight];
                match filter {
                    Filter::Linear => {}
                    Filter::Srgb => {
                        for v in &mut rgb {
                            *v = bevy::color::Srgba::gamma_function_inverse(if alpha > 0.0 {
                                *v / alpha
                            } else {
                                0.0
                            });
                        }
                    }
                    Filter::Normal => {
                        let normal = bevy::math::Vec3::from_array(rgb)
                            .try_normalize()
                            .unwrap_or(bevy::math::Vec3::Z);
                        rgb = (normal * 0.5 + bevy::math::Vec3::splat(0.5)).to_array();
                    }
                }
                let encode = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
                let out = [
                    encode(rgb[0]),
                    encode(rgb[1]),
                    encode(rgb[2]),
                    encode(alpha),
                ];
                data.extend_from_slice(&out);
            }
        }
        src_offset = dst_offset;
    }

    debug_assert_eq!(data.len(), total);
    (levels, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_layout_and_sizes() {
        // 4x2 -> levels: 4x2, 2x1, 1x1
        let mip0 = vec![128u8; 4 * 2 * 4];
        let (levels, data) = rgba8_mip_chain(4, 2, &mip0);
        assert_eq!(levels, 3);
        assert_eq!(data.len(), (4 * 2 + 2 * 1 + 1 * 1) * 4);
        // constant input stays constant through the box filter
        assert!(data.iter().all(|&b| b == 128));
    }

    #[test]
    fn srgb_average_is_linear_light_and_keeps_alpha_linear() {
        let (_, data) = rgba8_srgb_mip_chain(2, 1, &[0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(&data[8..], &[188, 188, 188, 255]);
        let (_, linear) = rgba8_mip_chain(2, 1, &[0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(&linear[8..], &[128, 128, 128, 255]);
        let (_, edge) = rgba8_srgb_mip_chain(2, 1, &[255, 255, 255, 255, 0, 0, 0, 0]);
        assert_eq!(&edge[8..], &[255, 255, 255, 128]);
    }

    #[test]
    fn odd_edges_contribute_to_the_final_average() {
        // Only the last row/column carries colour: the old 2x2 filter lost both.
        let (_, row) = rgba8_mip_chain(3, 1, &[0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(&row[12..], &[85, 85, 85, 255]);
        let mut pixels = vec![0u8; 5 * 3 * 4];
        for p in pixels.chunks_exact_mut(4) {
            p[3] = 255;
        }
        pixels[56..60].copy_from_slice(&[255, 255, 255, 255]);
        let (_, chain) = rgba8_mip_chain(5, 3, &pixels);
        assert_eq!(&chain[chain.len() - 4..], &[17, 17, 17, 255]);
    }

    #[test]
    fn single_texel_is_one_level() {
        let mip0 = vec![7u8; 4];
        let (levels, data) = rgba8_mip_chain(1, 1, &mip0);
        assert_eq!(levels, 1);
        assert_eq!(data, mip0);
    }

    #[test]
    fn npot_dims_floor() {
        // 5x3 -> 5x3, 2x1, 1x1
        let mip0 = vec![0u8; 5 * 3 * 4];
        let (levels, data) = rgba8_mip_chain(5, 3, &mip0);
        assert_eq!(levels, 3);
        assert_eq!(data.len(), (5 * 3 + 2 * 1 + 1 * 1) * 4);
    }

    #[test]
    fn normal_average_renormalizes() {
        // opposing x-tilted normals average to a short vector under a plain
        // box filter; the normal-map path renormalizes back to unit length
        let a = [200, 128, 180, 255];
        let b = [56, 128, 180, 255];
        let (_, chain) = rgba8_normal_mip_chain(2, 2, &[a, b, a, b].concat());
        let out = &chain[16..20];
        let d = |c: u8| c as f32 / 255.0 * 2.0 - 1.0;
        let len = (d(out[0]).powi(2) + d(out[1]).powi(2) + d(out[2]).powi(2)).sqrt();
        assert!((len - 1.0).abs() < 0.02, "not renormalized: len = {len}");
    }
}
