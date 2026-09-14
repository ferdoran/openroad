//! Generates the foliage pack cards (`assets/foliage/grass_{a..d}.png`) from
//! the ambientCG "Foliage006" grass-blade atlas (CC0).
//!
//! The atlas is a scan of ~10 individual blades with the alpha in a separate
//! greyscale `_Opacity.png`. A single blade makes a sparse, lonely billboard,
//! so each output card is *composed*: the opacity is merged into the color's
//! alpha, blades are isolated as connected alpha islands, and a seeded RNG
//! fans several blades around a common root at the bottom center of a 512x512
//! canvas — producing a tuft silhouette like hand-authored grass cards. The
//! seed is fixed, so the cards are reproducible byte-for-byte.
//!
//! Usage:
//!   cargo run -p tools --bin gen_foliage_cards -- --input <dir with
//!     Foliage006_2K-PNG_Color.png + Foliage006_2K-PNG_Opacity.png>
//!     [--out assets/foliage]

use image::{imageops, GrayImage, Rgba, RgbaImage};

const CARD_SIZE: u32 = 512;
const CARDS: [&str; 4] = ["grass_a.png", "grass_b.png", "grass_c.png", "grass_d.png"];
/// Blades per composed tuft, per card (varied for silhouette diversity).
const BLADES_PER_CARD: [usize; 4] = [7, 5, 9, 6];
/// Alpha threshold for island detection.
const ALPHA_THRESHOLD: u8 = 32;
/// Fixed seed: cards are reproducible.
const SEED: u64 = 0x0F0114_6E;

fn main() {
    let mut input = None;
    let mut out = String::from("assets/foliage");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--input" => input = args.next(),
            "--out" => out = args.next().expect("--out needs a value"),
            other => panic!("unknown argument {other}"),
        }
    }
    let input = input.expect("--input <dir with Foliage006_2K-PNG_{Color,Opacity}.png> required");

    let color = image::open(format!("{input}/Foliage006_2K-PNG_Color.png"))
        .expect("color map")
        .to_rgba8();
    let opacity = image::open(format!("{input}/Foliage006_2K-PNG_Opacity.png"))
        .expect("opacity map")
        .to_luma8();
    assert_eq!(color.dimensions(), opacity.dimensions());

    // composite the separate opacity map into the color alpha
    let mut atlas = color;
    for (pixel, alpha) in atlas.pixels_mut().zip(opacity.pixels()) {
        pixel.0[3] = alpha.0[0];
    }

    // keep only slender near-vertical blades: the atlas also contains bent /
    // multi-blade clusters whose roots aren't at their bottom-center, and
    // those read as floating fragments once fanned around a common root
    let blades: Vec<RgbaImage> = find_islands(&atlas)
        .into_iter()
        .filter(|b| b.height() >= b.width() * 4)
        .collect();
    println!(
        "found {} blades (min {}x{}, max {}x{})",
        blades.len(),
        blades.iter().map(|b| b.width()).min().unwrap_or(0),
        blades.iter().map(|b| b.height()).min().unwrap_or(0),
        blades.iter().map(|b| b.width()).max().unwrap_or(0),
        blades.iter().map(|b| b.height()).max().unwrap_or(0),
    );
    assert!(blades.len() >= 4, "atlas produced too few blade islands");

    let mut rng = SEED;
    std::fs::create_dir_all(&out).expect("output dir");
    for (card_idx, name) in CARDS.iter().enumerate() {
        let card = compose_card(&blades, BLADES_PER_CARD[card_idx], &mut rng);
        let path = format!("{out}/{name}");
        card.save(&path).expect("write card");
        println!("wrote {path}");
    }
    println!("source: ambientCG Foliage006 (CC0) — https://ambientcg.com/view?id=Foliage006");
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit(rng: &mut u64) -> f32 {
    (splitmix64(rng) >> 40) as f32 / (1u64 << 24) as f32
}

/// Connected alpha islands (4-neighbor flood fill), cropped to their bounds.
/// Tiny specks are dropped.
fn find_islands(atlas: &RgbaImage) -> Vec<RgbaImage> {
    let (w, h) = atlas.dimensions();
    let mut mask = GrayImage::new(w, h);
    let mut islands = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if mask.get_pixel(x, y).0[0] != 0 || atlas.get_pixel(x, y).0[3] < ALPHA_THRESHOLD {
                continue;
            }
            // flood fill
            let mut stack = vec![(x, y)];
            let mut pixels = Vec::new();
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (x, y, x, y);
            while let Some((px, py)) = stack.pop() {
                if mask.get_pixel(px, py).0[0] != 0
                    || atlas.get_pixel(px, py).0[3] < ALPHA_THRESHOLD
                {
                    continue;
                }
                mask.put_pixel(px, py, image::Luma([255]));
                pixels.push((px, py));
                min_x = min_x.min(px);
                min_y = min_y.min(py);
                max_x = max_x.max(px);
                max_y = max_y.max(py);
                if px > 0 {
                    stack.push((px - 1, py));
                }
                if py > 0 {
                    stack.push((px, py - 1));
                }
                if px + 1 < w {
                    stack.push((px + 1, py));
                }
                if py + 1 < h {
                    stack.push((px, py + 1));
                }
            }
            let (bw, bh) = (max_x - min_x + 1, max_y - min_y + 1);
            if bw.max(bh) < 64 {
                continue; // speck
            }
            let mut island = RgbaImage::new(bw, bh);
            for (px, py) in pixels {
                island.put_pixel(px - min_x, py - min_y, *atlas.get_pixel(px, py));
            }
            islands.push(island);
        }
    }
    islands
}

/// Fan `count` random blades around a common root at the bottom center.
fn compose_card(blades: &[RgbaImage], count: usize, rng: &mut u64) -> RgbaImage {
    let mut card = RgbaImage::new(CARD_SIZE, CARD_SIZE);
    let root = (CARD_SIZE as f32 / 2.0, CARD_SIZE as f32 - 6.0);
    for _ in 0..count {
        let blade = &blades[(splitmix64(rng) % blades.len() as u64) as usize];
        // blades are near-vertical; scale to a target height, lean outward
        let target_h = 300.0 + unit(rng) * 180.0;
        let scale = target_h / blade.height() as f32;
        let angle = (unit(rng) - 0.5) * 2.0 * 28f32.to_radians();
        let root_jitter = (unit(rng) - 0.5) * 70.0;
        stamp_blade(
            &mut card,
            blade,
            scale,
            angle,
            (root + Vec2::new(root_jitter, 0.0)).into(),
        );
    }
    card
}

#[derive(Clone, Copy)]
struct Vec2 {
    x: f32,
    y: f32,
}
impl Vec2 {
    fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}
impl std::ops::Add<Vec2> for (f32, f32) {
    type Output = (f32, f32);
    fn add(self, v: Vec2) -> (f32, f32) {
        (self.0 + v.x, self.1 + v.y)
    }
}

/// Alpha-over `blade`, scaled and rotated about its bottom-center, with the
/// bottom-center placed at `root` — inverse-mapped nearest sampling (the
/// cards are minified heavily in-game, so bilinear isn't worth the code).
fn stamp_blade(card: &mut RgbaImage, blade: &RgbaImage, scale: f32, angle: f32, root: (f32, f32)) {
    let scaled_w = (blade.width() as f32 * scale).max(1.0) as u32;
    let scaled_h = (blade.height() as f32 * scale).max(1.0) as u32;
    let scaled = imageops::resize(blade, scaled_w, scaled_h, imageops::FilterType::CatmullRom);
    let pivot = (scaled_w as f32 / 2.0, scaled_h as f32);
    let (sin, cos) = angle.sin_cos();
    for cy in 0..CARD_SIZE {
        for cx in 0..CARD_SIZE {
            // card -> blade space: translate to root, inverse-rotate, add pivot
            let dx = cx as f32 - root.0;
            let dy = cy as f32 - root.1;
            let bx = cos * dx + sin * dy + pivot.0;
            let by = -sin * dx + cos * dy + pivot.1;
            if bx < 0.0 || by < 0.0 || bx >= scaled_w as f32 || by >= scaled_h as f32 {
                continue;
            }
            let src = *scaled.get_pixel(bx as u32, by as u32);
            if src.0[3] == 0 {
                continue;
            }
            let dst = card.get_pixel_mut(cx, cy);
            *dst = alpha_over(src, *dst);
        }
    }
}

fn alpha_over(src: Rgba<u8>, dst: Rgba<u8>) -> Rgba<u8> {
    let sa = src.0[3] as f32 / 255.0;
    let da = dst.0[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        return Rgba([0, 0, 0, 0]);
    }
    let mut out = [0u8; 4];
    for i in 0..3 {
        let s = src.0[i] as f32 / 255.0;
        let d = dst.0[i] as f32 / 255.0;
        out[i] = (((s * sa + d * da * (1.0 - sa)) / out_a) * 255.0).round() as u8;
    }
    out[3] = (out_a * 255.0).round() as u8;
    Rgba(out)
}
