//! Intro chrome: the two full-width bars and the notice line that frame every
//! intro state.
//!
//! Idea: the bars are `GDR_STA_SCREENUP` / `GDR_STA_SCREENDOWN` (ids 1/2),
//! declared once per intro tree — and **each tree names its own art**, which is
//! why one handle cannot serve the whole scene:
//!
//! | state | tree | up / down art |
//! |---|---|---|
//! | title (splash, login, servers) | `pstitle_europe.txt:729/710` | `blackbar_up_18_europe` / `blackbar_down_copyright_europe` |
//! | character list + region select | `pscharacterselect_europe.txt:1010/991` | `blackbar_up_europe` / `blackbar_down_europe` |
//! | character create | `pscharactercreate_europe.txt:253/234` | `redbar_up_europe` / `redbar_down_europe` |
//!
//! A sweep for `blackbar` alone finds 13 references and none in the create
//! trees, which reads as "the create screen has no chrome". It has — in red
//! (`docs/re/ui/intro-chrome.md` §3). The classified sweep over all 247 resinfo
//! files returns 16 `interface\outer\*bar*` references: 13 `blackbar` + 4
//! `redbar`. All six arts are 1600x172 ARGB1555 and md5-distinct.
//!
//! `#ifdef` resolution is unambiguous: `define.txt:7` defines `EUROPE_SYSTEM`
//! and `APPLY_GNGWC_SYSTEM_2007` is absent from its 22 symbols, so `pstitle.txt`
//! resolves to exactly the pre-resolved `pstitle_europe.txt` pair.
//!
//! Because the bars are spawned once for the whole scene, the art is swapped on
//! state change ([`update_chrome_art`]) rather than re-spawned.

use bevy::prelude::*;

use super::assets::IntroV2Assets;
use super::IntroV2State;

/// Bar rects, verbatim: `Rect="0,0,1600,172"` and `Rect="0,1030,1600,172"` in
/// the trees' 1600x1200 design space. Height is expressed as a percentage of
/// that space so the bars scale with the window, as they did before; 1030+172
/// overruns the 1200 canvas by 2px, which is why the bottom bar is anchored to
/// the bottom edge instead of to y=1030.
const DESIGN_H: f32 = 1200.0;
const BAR_H: f32 = 172.0;
/// 172/1200 = 14.33%. The previous 15% was unsourced and 8px too tall at the
/// design size.
const BAR_H_PCT: f32 = 100.0 * BAR_H / DESIGN_H;

/// Notice-line colour: `GDR_TEXT_MESSAGE` (`CIFTextBox`, id 500)
/// `FontColor="255,255,103,29"` — resinfo COLOR is ARGB, so RGB(255,103,29).
/// Byte-exact; do not "fix" it against the editor-scratch `Color=` on the same
/// block (`255,23,9,242`).
const NOTICE_COLOR: Color = Color::srgb_u8(255, 103, 29);
/// **openroad choice.** Id 500's `Rect="0,0,0,0"` — the notice line's placement
/// and size are code-side in the original, and `FontIndex=0` carries no size
/// (the lane-wide FontIndex -> face mapping is UNKNOWN). Both numbers below are
/// ours, kept as they were rather than re-invented.
const NOTICE_FONT_SIZE: f32 = 16.0;
const NOTICE_BOTTOM_PCT: f32 = 10.0;

/// Marker for the top bar.
#[derive(Component, Default, Clone)]
pub struct HeaderV2;

/// Marker for the bottom bar.
#[derive(Component, Default, Clone)]
pub struct FooterV2;

/// Marker for the notice line at the bottom of the screen.
#[derive(Component, Default, Clone)]
pub struct InfoTextV2;

/// Replaces the notice line's content.
#[derive(Message)]
pub struct InfoTextV2Update(pub String);

/// Which intro tree frames a given state (see the table in the module doc).
/// Split out of the asset lookup so the mapping itself is testable.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BarTree {
    /// `pstitle_europe.txt:729/710`
    Title,
    /// `pscharacterselect_europe.txt:1010/991` — the list and the region board
    /// are both sections of that one tree
    Select,
    /// `pscharactercreate_europe.txt:253/234` — red, not black
    Create,
}

pub fn bar_tree(state: IntroV2State) -> BarTree {
    match state {
        IntroV2State::CharacterCreate => BarTree::Create,
        IntroV2State::CharacterList | IntroV2State::RegionSelect => BarTree::Select,
        IntroV2State::Loading
        | IntroV2State::Splash
        | IntroV2State::LoginForm
        | IntroV2State::ServerSelection => BarTree::Title,
    }
}

fn bar_art(state: IntroV2State, assets: &IntroV2Assets) -> (Handle<Image>, Handle<Image>) {
    match bar_tree(state) {
        BarTree::Create => (assets.redbar_up.clone(), assets.redbar_down.clone()),
        BarTree::Select => (assets.blackbar_up.clone(), assets.blackbar_down.clone()),
        BarTree::Title => (assets.title_bar_up.clone(), assets.title_bar_down.clone()),
    }
}

pub fn header(assets: &IntroV2Assets) -> impl Scene {
    // spawned in Loading; the title art is what the first visible state
    // (Splash) declares, and update_chrome_art swaps it from there on
    let image = assets.title_bar_up.clone();
    bsn! {
        HeaderV2
        Name("Header V2")
        ImageNode { image: {image}, image_mode: NodeImageMode::Stretch }
        Node {
            position_type: PositionType::Absolute,
            left: px(0),
            right: px(0),
            height: percent(BAR_H_PCT),
            top: px(0),
        }
        Pickable::IGNORE
    }
}

pub fn footer(assets: &IntroV2Assets) -> impl Scene {
    let image = assets.title_bar_down.clone();
    bsn! {
        FooterV2
        Name("Footer V2")
        ImageNode { image: {image}, image_mode: NodeImageMode::Stretch }
        Node {
            position_type: PositionType::Absolute,
            left: px(0),
            right: px(0),
            height: percent(BAR_H_PCT),
            bottom: px(0),
        }
        Pickable::IGNORE
    }
}

pub fn info_text() -> impl Scene {
    bsn! {
        InfoTextV2
        Name("Info Text V2")
        Text("")
        TextFont { font_size: {FontSize::Px(NOTICE_FONT_SIZE)} }
        TextColor(NOTICE_COLOR)
        TextLayout::justify(Justify::Center)
        Node {
            position_type: PositionType::Absolute,
            bottom: percent(NOTICE_BOTTOM_PCT),
            width: percent(100),
            justify_content: JustifyContent::Center,
        }
        Pickable::IGNORE
    }
}

/// Swap the bars to the art the current intro state's tree declares. The bars
/// are spawned once for the whole scene, so without this the create screen
/// keeps the select screen's black bars where the data says red.
pub fn update_chrome_art(
    // Option: the sub-state resource only exists while the intro scene runs
    state: Option<Res<State<IntroV2State>>>,
    assets: Option<Res<IntroV2Assets>>,
    mut headers: Query<&mut ImageNode, (With<HeaderV2>, Without<FooterV2>)>,
    mut footers: Query<&mut ImageNode, (With<FooterV2>, Without<HeaderV2>)>,
) {
    let (Some(state), Some(assets)) = (state, assets) else {
        return;
    };
    let (up, down) = bar_art(**state, &assets);
    for mut image in headers.iter_mut() {
        if image.image != up {
            image.image = up.clone();
        }
    }
    for mut image in footers.iter_mut() {
        if image.image != down {
            image.image = down.clone();
        }
    }
}

pub fn update_info_text(
    mut reader: MessageReader<InfoTextV2Update>,
    mut query: Query<&mut Text, With<InfoTextV2>>,
) {
    let Ok(mut text) = query.single_mut() else {
        return;
    };

    for update in reader.read() {
        text.0 = update.0.clone();
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The bar height is the authored one, not a round percentage: 172 of the
    /// 1600x1200 design space. The old 15% was unsourced and 8px too tall.
    #[test]
    fn the_bar_height_is_the_authored_rect() {
        assert_eq!(BAR_H, 172.0);
        assert!((BAR_H_PCT - 14.3333).abs() < 0.001);
        assert_ne!(BAR_H_PCT, 15.0);
        // the authored bottom bar starts at 1030 and overruns the canvas by 2px
        assert_eq!(1030.0 + BAR_H - DESIGN_H, 2.0);
    }

    /// Three trees, three bar pairs — and the create screen's are RED. A
    /// `blackbar` sweep alone misses that entirely, and the two black pairs
    /// are different arts, so "black for everything but create" is also wrong.
    #[test]
    fn each_intro_state_gets_its_own_trees_art() {
        assert_eq!(bar_tree(IntroV2State::CharacterCreate), BarTree::Create);
        assert_eq!(bar_tree(IntroV2State::CharacterList), BarTree::Select);
        assert_eq!(bar_tree(IntroV2State::RegionSelect), BarTree::Select);
        for state in [
            IntroV2State::Loading,
            IntroV2State::Splash,
            IntroV2State::LoginForm,
            IntroV2State::ServerSelection,
        ] {
            assert_eq!(bar_tree(state), BarTree::Title, "{state:?}");
        }
        // the title and select trees name DIFFERENT blackbar variants
        // (blackbar_up_18_europe vs blackbar_up_europe), so they are two
        // trees, not one shared pair
        assert_ne!(
            bar_tree(IntroV2State::Splash),
            bar_tree(IntroV2State::CharacterList)
        );
    }

    /// The notice colour is the block's `FontColor` (ARGB), not its `Color=`.
    #[test]
    fn the_notice_colour_is_the_fontcolor_not_the_scratch_color() {
        assert_eq!(NOTICE_COLOR, Color::srgb_u8(255, 103, 29));
        // Color="255,23,9,242" on the same block is editor scratch
        assert_ne!(NOTICE_COLOR, Color::srgb_u8(23, 9, 242));
    }
}
