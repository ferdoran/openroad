//! Character-creation sub-screen of the intro v2 flow.
//!
//! Idea: reuse the char-select 3d stage (same camera, world origin and model
//! spawn path) to show a *live* preview of the picked starter body + gear, and
//! drive the whole screen off a single [`CharCreateSelection`] resource — a
//! change to it re-spawns the preview and re-highlights the toggles, mirroring
//! how the char-select selection UI keys off `SelectedCharacterV2`. On submit it
//! sends the `Create` lobby action (with `CheckName` for a pre-submit
//! availability probe) over the still-open agent connection and, on success,
//! returns to a freshly-listed `CharacterList`.

use std::f32::consts::PI;

use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use bevy::text::EditableText;
use bevy::ui::{InteractionDisabled, Pressed};
use bevy::ui_widgets::Activate;

use packets::agent::prelude::*;
use packets::Packet;

use crate::net::connection::SilkroadConnection;
use crate::plugins::camera::CinematicCamera2;
use crate::plugins::dynamic_resource_loader::{MirroredResource, UnloadedResource};
use crate::plugins::net::agent::AgentConnection;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::{
    ClientCharacterData, ClientItemData, ClientItemIndex, ClientUiStrings,
};
use crate::plugins::ui_v2::style::{ButtonSound, ImageButtonStyle};
use crate::plugins::ui_v2::widgets::{image_button, label, text_input};
use crate::plugins::world_origin::WorldOrigin;
use crate::util::mesh::needs_winding_reversal;

use super::assets::IntroV2Assets;
use super::character_select;
use super::chrome::InfoTextV2Update;
use super::login_form::main_button_style;
use super::scene_data::ActiveCharSelectSceneV2;
use super::{IntroV2State, IntroV2Ui};
use crate::assets::FontAssets;

/// Marker resource set by the char-select Create button so the char-list exit
/// keeps the agent connection alive for the switch into creation.
#[derive(Resource)]
pub struct EnteringCharacterCreate;

/// The two v1.188 playable races (Islam/Arabia is not a v1.188 body).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Race {
    #[default]
    Chinese,
    European,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Gender {
    #[default]
    Male,
    Female,
}

/// The current creation choice; its change drives the live preview, the
/// toggle highlights and the picker labels. `figure`/`weapon` index into the
/// corpus-enumerated option lists (clamped on resolve).
#[derive(Resource, Default)]
pub struct CharCreateSelection {
    pub race: Race,
    pub gender: Gender,
    pub figure: usize,
    pub weapon: usize,
    pub garment: Garment,
}

/// Root marker of the creation UI (left panel + bottom button row).
#[derive(Component, Default, Clone)]
pub struct CharCreateRoot;

/// The 3d body preview reflecting the current selection.
#[derive(Component, Default, Clone)]
pub struct CharCreatePreview;

/// How the preview is being looked at, driven by the original's
/// `Section = Rotate` controls: an accumulated yaw and the zoom toggle.
/// Kept as a resource (not on the preview entity) because the preview is
/// re-spawned on every selection change and the view must survive that.
#[derive(Resource, Default)]
pub struct CharCreatePreviewView {
    /// Yaw added to the podium pose, in radians.
    pub yaw: f32,
    /// `GDR_BTN_ZOOM` state: the art swaps `zoomin` <-> `zoomout`.
    pub zoomed_in: bool,
}

/// One of the two rotate buttons; the sign is the yaw direction. `Default`
/// is required by `bsn!` (every templated component must be constructible)
/// and is never the value used: both call sites pass their own step.
#[derive(Component, Clone, Default)]
pub struct PreviewRotateButton(pub f32);

/// `GDR_BTN_ZOOM` — its texture set is swapped on toggle.
#[derive(Component, Default, Clone)]
pub struct PreviewZoomButton;

/// Marker on the name `EditableText`.
#[derive(Component, Default, Clone)]
pub struct NameInput;

/// Marker on the name-availability feedback text.
#[derive(Component, Default, Clone)]
pub struct NameFeedback;

/// Marker on the Create button, used to re-enable it when the server rejects.
#[derive(Component, Default, Clone)]
pub struct CreateButton;

/// Root of the `Section = WCreate` confirmation modal.
#[derive(Component, Default, Clone)]
pub struct CreateConfirmModal;

/// Marker on a gender toggle *button*. It sits on the button and not on its
/// caption because the original expresses selection as a texture swap
/// (`man_on` <-> `man_off`), which is a property of the button.
#[derive(Component, Default, Clone)]
pub struct GenderOption(pub Gender);

/// Which option row a `CIFSliderCtrl` instance drives. The original authors
/// `Section = Slider` ONCE and instantiates it per row, so the row identity is
/// a component on the instance rather than five separate widgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Component)]
pub enum SliderRow {
    #[default]
    Figure,
    Protector,
    Weapon,
}

/// A slider's prev/next arrow: which row it drives and in which direction.
#[derive(Component, Default, Clone)]
pub struct SliderStep(pub SliderRow, pub i32);

/// A slider's `GDR_BTN_THUMB`; its `left` tracks the row's current index.
#[derive(Component, Default, Clone)]
pub struct SliderThumb(pub SliderRow);

/// The row the Explain box is currently describing. The original's Explain
/// panel is context-sensitive per focused slider (24 `UIO_NEWCHAR_EXPLANATION_*`
/// keys ship for exactly that), which is also why the rows carry no value text
/// of their own: the readout IS the Explain box.
#[derive(Resource, Default)]
pub struct CharCreateFocus(pub SliderRow);

/// Marker on the figure-story title (the figure's display name).
#[derive(Component, Default, Clone)]
pub struct FigureStoryTitle;

/// Marker on the figure-story body (the vanilla `*_EXPLANATION` text).
#[derive(Component, Default, Clone)]
pub struct FigureStoryText;

/// Dimmed caption of a data-blocked option.
/// `GDR_STA_TITLE` art (`text-custom.ddj`), `Rect=47,111,428,36` in both the
/// CH and EU create trees.
const TITLE_CUSTOM_DDJ: &str = "media://interface/outer/text-custom.ddj";
const TITLE_ART_W: f32 = 428.0;
const TITLE_ART_H: f32 = 36.0;

const DISABLED_COLOR: Color = Color::srgb(0.45, 0.45, 0.45);

// --- `GDR_STA_CUSTOM` and its `Section = Custom` children --------------------
//
// Idea: the customize panel is a fixed-size baked sprite
// (`customize_window[_europe].ddj`, 264x316) whose slots — the name field, the
// two sex buttons, the five option rows — are painted INTO the art. So the
// children are absolutely placed at the resinfo rects in the panel's own pixel
// space and nothing here is flex, padding or spacing that we get to choose.
// Line numbers below are from `resinfo/pscharactercreatechina.txt`; the
// European file is line-for-line identical except where noted.

/// `GDR_STA_CUSTOM` (`:139-157`), `customize_window[_europe].ddj` — both race
/// variants are 264x316.
const CUSTOM_W: f32 = 264.0;
const CUSTOM_H: f32 = 316.0;
/// `GDR_STATIC1` (`:561-579`) `UIO_NEWCHAR_STT_NAME` and `GDR_EDIT_NAME`
/// (`:428-446`). The edit has no DDJ: its slot is baked into the panel art.
const NAME_LABEL_RECT: (f32, f32, f32, f32) = (32.0, 35.0, 45.0, 15.0);
const NAME_EDIT_RECT: (f32, f32, f32, f32) = (92.0, 32.0, 135.0, 20.0);
/// `GDR_BTN_CHECK` (`:409-427`) on `overlap.ddj` (75x25, exact fit).
const CHECK_RECT: (f32, f32, f32, f32) = (156.0, 57.0, 75.0, 25.0);
/// `GDR_STATIC2` (`:542-560`) `UIO_NEWCHAR_STT_SEX`, then `GDR_BTN_MALE`
/// (`:390-408`) and `GDR_BTN_FEMALE` (`:371-389`).
const SEX_LABEL_RECT: (f32, f32, f32, f32) = (32.0, 100.0, 45.0, 15.0);
const MALE_RECT: (f32, f32, f32, f32) = (87.0, 95.0, 70.0, 25.0);
const FEMALE_RECT: (f32, f32, f32, f32) = (166.0, 95.0, 70.0, 25.0);
/// The five `CIFSliderCtrl` rows all share `x=88, w=120, h=24` and step by 31:
/// `GDR_SLI_FIGURE :352-370`, `_HEIGHT :333-351`, `_VOLUME :314-332`, and the
/// two gear rows (see [`gear_row_tops`]).
const ROW_X: f32 = 88.0;
const ROW_W: f32 = 120.0;
const ROW_H: f32 = 24.0;
const ROW_FIGURE_TOP: f32 = 131.0;
const ROW_HEIGHT_TOP: f32 = 162.0;
const ROW_VOLUME_TOP: f32 = 193.0;
/// The two gear rows, in the order the *Chinese* tree authors them.
const ROW_GEAR_TOPS: (f32, f32) = (224.0, 255.0);
/// Row captions `GDR_STATIC3..7` (`:447-541`), all `x=32, w=45, h=15`.
const LABEL_X: f32 = 32.0;
const LABEL_W: f32 = 45.0;
const LABEL_H: f32 = 15.0;
const LABEL_FIGURE_TOP: f32 = 137.0;
const LABEL_HEIGHT_TOP: f32 = 167.0;
const LABEL_VOLUME_TOP: f32 = 198.0;
const LABEL_GEAR_TOPS: (f32, f32) = (229.0, 260.0);
/// Name-check feedback line. `GDR_TEXT_MESSAGE` (`:196-214`) has rect
/// `0,0,0,0` — the original places its message sink from code (UNKNOWN).
/// **openroad choice:** the panel's free strip below the last row (which ends
/// at 260+15=275), so the answer appears next to the field it is about.
const FEEDBACK_TOP: f32 = 285.0;
const FEEDBACK_W: f32 = 200.0;

// --- `Section = Slider` (`:685-744`) — one template, every row ---------------
//
// Idea: the tree authors the slider ONCE and each `CIFSliderCtrl` row
// instantiates it, so we build one widget and place it per row rather than
// five bespoke pickers. All three child rects are in the template's own space.
//
// Note the width: the row's own `Rect` is 120 wide, but `GDR_BTN_NEXT` starts
// at x=120, so a drawn instance is 140 wide. That is not a contradiction to
// paper over — the panel is 264 wide and the rows start at x=88, so 88+140=228
// still lands inside it, which is what makes the 140 credible.

/// `GDR_BTN_PREV` (`:725-743`), `arrow_left.ddj`.
const SLIDER_PREV_RECT: (f32, f32, f32, f32) = (0.0, 2.0, 20.0, 20.0);
/// `GDR_BTN_THUMB` (`:687-705`), `slider.ddj` — the rect at index 0.
const SLIDER_THUMB_RECT: (f32, f32, f32, f32) = (20.0, 0.0, 16.0, 24.0);
/// `GDR_BTN_NEXT` (`:706-724`), `arrow_right.ddj`.
const SLIDER_NEXT_RECT: (f32, f32, f32, f32) = (120.0, 2.0, 20.0, 20.0);
/// Drawn width of one instance: up to the right edge of the next arrow.
const SLIDER_TEMPLATE_W: f32 = SLIDER_NEXT_RECT.0 + SLIDER_NEXT_RECT.2;

/// Where the thumb sits for `index` of `count` steps.
///
/// Derived, not invented: the thumb starts at the authored `x=20` and the
/// track ends where `GDR_BTN_NEXT` begins (`x=120`), so the thumb's travel is
/// `120 - 20 - 16 = 84` px and the steps divide it evenly. A single-step row
/// (or an empty one) parks the thumb at the start.
fn slider_thumb_left(index: usize, count: usize) -> f32 {
    let start = SLIDER_THUMB_RECT.0;
    if count <= 1 {
        return start;
    }
    let travel = SLIDER_NEXT_RECT.0 - start - SLIDER_THUMB_RECT.2;
    start + travel * index.min(count - 1) as f32 / (count - 1) as f32
}

// --- `Section = WCreate` — the create-confirm modal --------------------------
//
// Idea: the same shape char-select's delete-confirm already has — a dimming
// full-screen scrim (which is what actually makes the modal modal: it eats the
// clicks behind it) carrying the warning frame at its native size. Only the
// art and the child rects differ, so the button style is shared rather than
// re-minted.

/// `GDR_STA_WCREATE` (`:25-43`), `warning_create.ddj` at its native size.
const WCREATE_W: f32 = 248.0;
const WCREATE_H: f32 = 128.0;
/// `GDR_STA_WNAME` (`:805-823`) — the name being created, FontIndex 2.
const WCREATE_NAME_RECT: (f32, f32, f32, f32) = (4.0, 23.0, 239.0, 15.0);
/// `GDR_STATIC1` (`:786-804`), `UIO_NEWCHAR_MSG_CREATE`.
const WCREATE_MSG_RECT: (f32, f32, f32, f32) = (4.0, 47.0, 239.0, 13.0);
/// `GDR_BTN_WCREATE` (`:767-785`) `UIO_COMMON_CTL_CREATE` — this is where that
/// key belongs; the screen's own `GDR_BTN_OK` carries `UIO_NEWCHAR_CTL_CONFIRM`
/// and a stale comment used to misattribute it to this button.
const WCREATE_OK_RECT: (f32, f32, f32, f32) = (42.0, 75.0, 76.0, 32.0);
/// `GDR_BTN_WCANCEL` (`:748-766`) `UIO_COMMON_CTL_CANCEL`.
const WCREATE_CANCEL_RECT: (f32, f32, f32, f32) = (130.0, 75.0, 76.0, 32.0);

/// `GDR_STA_EXPLAIN` (`:101-119`) and its `Section = Explain` children
/// (`:582-622`): the description panel right of the customize window.
/// **The one rect/art mismatch in the tree** (RE doc §9-U7): the European tree
/// declares `0,0,212,250` but ships `explain-window_02.ddj` at 220x236. We
/// draw the ART at its own size, because a stretched 9-slice-less sprite would
/// visibly distort; the declared rect is recorded here and not silently lost.
const EXPLAIN_CH_SIZE: (f32, f32) = (212.0, 180.0);
/// Recorded, not consumed: the declared European rect. Kept as a constant so
/// the mismatch stays in the source next to the art size we actually draw.
#[allow(dead_code)]
const EXPLAIN_EU_RECT: (f32, f32) = (212.0, 250.0);
const EXPLAIN_EU_ART: (f32, f32) = (220.0, 236.0);
/// `GDR_STA_EXPLAINNAME` (`:603-621`), FontColor `255,255,239,153`, and
/// `GDR_TEXT_EXPLAIN` (`:584-602`).
const EXPLAIN_NAME_RECT: (f32, f32, f32, f32) = (15.0, 18.0, 183.0, 14.0);
const EXPLAIN_BODY_RECT: (f32, f32, f32, f32) = (15.0, 42.0, 183.0, 122.0);
/// `GDR_STA_EXPLAINNAME`'s authored `FontColor` — a warm parchment, distinct
/// from the char-select gold this panel used to borrow.
const EXPLAIN_NAME_COLOR: Color = Color::srgb(1.0, 239.0 / 255.0, 153.0 / 255.0);

/// The gear rows swap between the race trees — this is behavioural fidelity,
/// not decoration. Chinese authors `GDR_SLI_PROTECTOR` at y=224 and
/// `GDR_SLI_WEAPON` at y=255 (`china :295-313` / `:276-294`); European swaps
/// them (`_europe :276-294` / `:295-313`), and its captions swap with them
/// (`GDR_STATIC6`/`7` texts, `:459` / `:478` in both files). Returns
/// `(protector_row_top, weapon_row_top)`.
fn gear_row_tops(race: Race, tops: (f32, f32)) -> (f32, f32) {
    match race {
        Race::Chinese => tops,
        Race::European => (tops.1, tops.0),
    }
}

/// `GDR_STA_ROTATE` (`pscharactercreate{china,_europe}.txt:120-138`): the
/// 152x56 `rotate_window.ddj` panel holding the preview controls. Both race
/// trees are byte-identical here, so there is no `_europe` variant to pick.
const ROTATE_WINDOW_W: f32 = 152.0;
const ROTATE_WINDOW_H: f32 = 56.0;
/// `Section = Rotate` (`:624-683`), child rects as authored, in the section's
/// own space: `GDR_BTN_LROTATE` (:664-682), `GDR_BTN_ZOOM` (:645-663),
/// `GDR_BTN_RROTATE` (:626-644). `(left, top, width, height)`.
const ROTATE_LEFT_RECT: (f32, f32, f32, f32) = (12.0, 11.0, 42.0, 35.0);
const ROTATE_ZOOM_RECT: (f32, f32, f32, f32) = (57.0, 11.0, 39.0, 35.0);
const ROTATE_RIGHT_RECT: (f32, f32, f32, f32) = (99.0, 11.0, 42.0, 35.0);

/// Yaw applied per rotate click. **Deviation / openroad choice:** the resinfo
/// tree authors the buttons but not what they do — no step, speed or limit
/// ships anywhere in the data (`docs/re/ui/scene-intro-character-create.md`
/// §9). 15 deg is a 24-click full turn, which reads as a deliberate step on
/// both mouse and keyboard rather than a spin.
const PREVIEW_ROTATE_STEP: f32 = PI / 12.0;
/// Dolly of the zoomed-in create camera along its own view axis, in world
/// units. **Deviation / openroad choice:** `zoomin.ddj`/`zoomout.ddj` prove
/// the control is a two-state toggle, but no distance ships. The create pose
/// stands ~44 units off the podium (`assets/char_selects/*.selection`:
/// `cam_offset.z 697.6` vs `char_*_offset.z 654/644`), so 15 units is a
/// readable close-up that cannot pass through the body.
const PREVIEW_ZOOM_DOLLY: f32 = 15.0;

/// Default body-scale byte sent on create: a live capture of an original-made
/// character carries 0x22 (packet_dump/0xb007.log), the untouched
/// height/volume slider default. The nibble packing behind the original
/// GDR_SLI_HEIGHT/GDR_SLI_VOLUME sliders is UNKNOWN; the sliders themselves
/// are tracked EP-10.1 remainder.
const DEFAULT_SCALE: u8 = 0x22;

/// Client-side name cap mirroring the original's pre-send validation
/// (UIO_MSG_ERROR_CHARACTER_NAME_STRING: "Only 12 English letters are
/// available"). The minimum bound is UNKNOWN — only emptiness is rejected.
const MAX_NAME_LEN: usize = 12;

// --- Creation data model -----------------------------------------------------
//
// Everything below is enumerated from the user's own PK2 tables at runtime,
// mirroring the original creation inputs (resinfo pscharactercreate*:
// GDR_SLI_FIGURE / GDR_SLI_WEAPON / GDR_SLI_PROTECTOR):
//
// - Bodies: the *figure variant* rows (CHAR_{CH,EU}_{MAN,WOMAN}_*, ids
//   1907–1932 for CH on this corpus) — no bare CHAR_XX_YY rows exist, and the
//   live capture in packet_dump/0xb007.log shows an original-made character
//   carrying variant ref 1907. Display names come from the
//   UIO_NEWCHAR_{EU_}{MAN,FEMALE}_<SUFFIX> textuisystem keys ("Ryujoyeong",
//   "Eungyo", …). Races whose prefix yields no rows (European on this corpus)
//   are presented disabled.
// - Weapons: the degree-1 `_DEF` creation weapons per race (corpus ids
//   3632–3636 CH / 10887–10896 EU), resolved by codename; entries missing
//   from the corpus drop out of the picker.
// - Protector: the three `_DEF` garment sets (CLOTHES / LIGHT / HEAVY) per
//   race/gender, again resolved by codename.
//
// The height/volume sliders stay display-only: the nibble packing of the
// scale byte is UNKNOWN (only the untouched-slider default 0x22 is
// capture-verified), and guessing wire values against the live server would
// mint permanently mis-scaled characters.

/// The three `_DEF` starter garment sets (original `GDR_SLI_PROTECTOR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Garment {
    #[default]
    Clothes,
    Light,
    Heavy,
}

impl Garment {
    pub const ALL: [Garment; 3] = [Garment::Clothes, Garment::Light, Garment::Heavy];

    /// Next/previous set, wrapping — the row is one cycling control, the way
    /// the original's single `GDR_SLI_PROTECTOR` slider is.
    fn cycle(self, step: isize) -> Garment {
        let len = Self::ALL.len() as isize;
        let index = Self::ALL.iter().position(|g| *g == self).unwrap_or(0) as isize;
        Self::ALL[(index + step).rem_euclid(len) as usize]
    }

    /// Codename segment in `ITEM_{race}_{gender}_<SEGMENT>_01_<slot>_A_DEF`.
    fn segment(self) -> &'static str {
        match self {
            Garment::Clothes => "CLOTHES",
            Garment::Light => "LIGHT",
            Garment::Heavy => "HEAVY",
        }
    }

    /// Caption key + fallback per race (textuisystem: STT_CLOTHES "Garment",
    /// STT_LIGHT_ARMOR, STT_HEAVY_ARMOR; EU variants incl. ROBE).
    fn label(self, race: Race) -> (&'static str, &'static str) {
        match (race, self) {
            (Race::Chinese, Garment::Clothes) => ("UIO_NEWCHAR_STT_CLOTHES", "Garment"),
            (Race::Chinese, Garment::Light) => ("UIO_NEWCHAR_STT_LIGHT_ARMOR", "Protector"),
            (Race::Chinese, Garment::Heavy) => ("UIO_NEWCHAR_STT_HEAVY_ARMOR", "Armor"),
            (Race::European, Garment::Clothes) => ("UIO_NEWCHAR_STT_EU_ROBE", "Robe"),
            (Race::European, Garment::Light) => ("UIO_NEWCHAR_STT_EU_LIGHT_ARMOR", "Light Armor"),
            (Race::European, Garment::Heavy) => ("UIO_NEWCHAR_STT_EU_HEAVY_ARMOR", "Heavy Armor"),
        }
    }
}

impl Race {
    fn body_prefix(self, gender: Gender) -> &'static str {
        match (self, gender) {
            (Race::Chinese, Gender::Male) => "CHAR_CH_MAN_",
            (Race::Chinese, Gender::Female) => "CHAR_CH_WOMAN_",
            (Race::European, Gender::Male) => "CHAR_EU_MAN_",
            (Race::European, Gender::Female) => "CHAR_EU_WOMAN_",
        }
    }

    fn item_tokens(self, gender: Gender) -> (&'static str, &'static str) {
        (
            match self {
                Race::Chinese => "CH",
                Race::European => "EU",
            },
            match gender {
                Gender::Male => "M",
                Gender::Female => "W",
            },
        )
    }

    /// Degree-1 creation weapons: (codename, caption key, fallback). The CH
    /// five and the EU list mirror the original GDR_SLI_WEAPON options; the
    /// per-weapon caption keys are the vanilla UIO_NEWCHAR_STT_* entries.
    fn weapon_options(self) -> &'static [(&'static str, &'static str, &'static str)] {
        match self {
            Race::Chinese => &[
                ("ITEM_CH_SWORD_01_A_DEF", "UIO_NEWCHAR_STT_SWORD", "Sword"),
                ("ITEM_CH_BLADE_01_A_DEF", "UIO_NEWCHAR_STT_BLADE", "Blade"),
                ("ITEM_CH_SPEAR_01_A_DEF", "UIO_NEWCHAR_STT_SPEAR", "Spear"),
                (
                    "ITEM_CH_TBLADE_01_A_DEF",
                    "UIO_NEWCHAR_STT_TBLADE",
                    "Glaive",
                ),
                ("ITEM_CH_BOW_01_A_DEF", "UIO_NEWCHAR_STT_BOW", "Bow"),
            ],
            Race::European => &[
                (
                    "ITEM_EU_SWORD_01_A_DEF",
                    "UIO_NEWCHAR_STT_EU_ONEHANDSWORD",
                    "One-hand Sword",
                ),
                (
                    "ITEM_EU_TSWORD_01_A_DEF",
                    "UIO_NEWCHAR_STT_EU_TWOHANDSWORD",
                    "Two-hand Sword",
                ),
                (
                    "ITEM_EU_AXE_01_A_DEF",
                    "UIO_NEWCHAR_STT_EU_DUELAXE",
                    "Dual Axe",
                ),
                (
                    "ITEM_EU_DAGGER_01_A_DEF",
                    "UIO_NEWCHAR_STT_EU_DAGGER",
                    "Dagger",
                ),
                (
                    "ITEM_EU_CROSSBOW_01_A_DEF",
                    "UIO_NEWCHAR_STT_EU_CROSSBOW",
                    "Crossbow",
                ),
                (
                    "ITEM_EU_DARKSTAFF_01_A_DEF",
                    "UIO_NEWCHAR_STT_EU_DARKSTAFF",
                    "Warlock Rod",
                ),
                (
                    "ITEM_EU_TSTAFF_01_A_DEF",
                    "UIO_NEWCHAR_STT_EU_TWOHANDSTAFF",
                    "Two-hand Staff",
                ),
                (
                    "ITEM_EU_STAFF_01_A_DEF",
                    "UIO_NEWCHAR_STT_EU_ONEHANDSTAFF",
                    "Cleric Rod",
                ),
                ("ITEM_EU_HARP_01_A_DEF", "UIO_NEWCHAR_STT_EU_HARP", "Harp"),
            ],
        }
    }
}

/// The figure-variant body rows for a (race, gender), ordered by ref id (the
/// data's own creation order — 1907.. = ADVENTURER first, matching the
/// capture). Empty when the corpus lacks the race (EU here). `pub(crate)`:
/// the region-select board uses it to grey out data-blocked races.
pub(crate) fn figure_variants(
    char_data: &ClientCharacterData,
    race: Race,
    gender: Gender,
) -> Vec<(i32, String)> {
    let Some(table) = char_data.data() else {
        return Vec::new();
    };
    let prefix = race.body_prefix(gender);
    let mut rows: Vec<(i32, String)> = table
        .iter()
        .filter(|(_, row)| row.code_name().starts_with(prefix))
        .map(|(id, row)| (*id, row.code_name().clone()))
        .collect();
    rows.sort_by_key(|(id, _)| *id);
    rows
}

/// textuisystem key of a figure's display name: codename suffix after the
/// body prefix, keyed as UIO_NEWCHAR_{EU_}{MAN|FEMALE}_<SUFFIX>.
fn figure_label_key(race: Race, gender: Gender, code_name: &str) -> String {
    let suffix = code_name
        .strip_prefix(race.body_prefix(gender))
        .unwrap_or(code_name);
    let race_part = match race {
        Race::Chinese => "",
        Race::European => "EU_",
    };
    let gender_part = match gender {
        Gender::Male => "MAN",
        Gender::Female => "FEMALE",
    };
    format!("UIO_NEWCHAR_{race_part}{gender_part}_{suffix}")
}

/// The weapon options of `race` that actually resolve on this corpus:
/// (ref id, caption key, fallback).
fn weapon_choices(
    race: Race,
    item_index: &ClientItemIndex,
) -> Vec<(i32, &'static str, &'static str)> {
    race.weapon_options()
        .iter()
        .filter_map(|(code, key, fallback)| item_index.id(code).map(|id| (id, *key, *fallback)))
        .collect()
}

fn garment_codename(race: Race, gender: Gender, set: Garment, slot: &str) -> String {
    let (r, g) = race.item_tokens(gender);
    format!("ITEM_{r}_{g}_{}_01_{slot}_A_DEF", set.segment())
}

/// Ref-obj-ids for the create request + preview.
struct StarterRefs {
    body: u32,
    chest: u32,
    pants: u32,
    boots: u32,
    weapon: u32,
}

/// Resolves the current selection to ref-obj-ids; `None` if the corpus lacks
/// the race's bodies, the picked garment set, or every weapon option. Indices
/// are clamped so a stale figure/weapon index (e.g. after a race switch)
/// still resolves.
fn resolve_starter(
    sel: &CharCreateSelection,
    char_data: &ClientCharacterData,
    item_index: &ClientItemIndex,
) -> Option<StarterRefs> {
    let figures = figure_variants(char_data, sel.race, sel.gender);
    let (body, _) = figures.get(sel.figure.min(figures.len().checked_sub(1)?))?;
    let weapons = weapon_choices(sel.race, item_index);
    let (weapon, _, _) = weapons.get(sel.weapon.min(weapons.len().checked_sub(1)?))?;
    let garment =
        |slot: &str| item_index.id(&garment_codename(sel.race, sel.gender, sel.garment, slot));
    Some(StarterRefs {
        body: *body as u32,
        chest: garment("BA")? as u32,
        pants: garment("LA")? as u32,
        boots: garment("FA")? as u32,
        weapon: *weapon as u32,
    })
}

/// `GDR_STA_CUSTOM`: the customize panel, drawn as the baked
/// `customize_window[_europe].ddj` sprite with every child absolutely placed
/// at its `Section = Custom` rect.
///
/// Idea: the panel art already paints the frames the children sit in, so the
/// only correct layout is the authored one — no flex, no padding, nothing to
/// tune. Two things vary by race and are read from the race's own tree rather
/// than blended: the panel texture, and the order of the two gear rows.
///
/// The five option rows keep our existing prev/value/next cycler *inside* the
/// authored 120x24 row rect. Replacing that cycler with the `Section = Slider`
/// template (`:685-745`) is the next build step and a separate change; this
/// one is the texture/rect pass.
fn create_panel(
    assets: &IntroV2Assets,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
    race: Race,
) -> impl Scene {
    let panel_art = match race {
        Race::Chinese => assets.customize_window.clone(),
        Race::European => assets.customize_window_europe.clone(),
    };
    let male_style = gender_button_style(assets, Gender::Male, true);
    let female_style = gender_button_style(assets, Gender::Female, false);
    let check_style = ImageButtonStyle {
        normal: assets.overlap.clone(),
        hover: assets.overlap_focus.clone(),
        press: assets.overlap_press.clone(),
        ..Default::default()
    };
    let male_font = fonts.nine.clone();
    let female_font = fonts.nine.clone();
    let name_font = fonts.nine.clone();
    let sex_font = fonts.nine.clone();
    let input_font = fonts.nine.clone();
    let check_font = fonts.nine.clone();
    let feedback_font = fonts.nine.clone();
    let male_sound = assets.sound_button_sound_a.clone();
    let female_sound = assets.sound_button_sound_a.clone();
    let check_sound = assets.sound_button_sound_a.clone();
    let male_text = ui_strings.get_or("UIO_NEWCHAR_CTL_MALE", "Male");
    let female_text = ui_strings.get_or("UIO_NEWCHAR_CTL_FEMALE", "Female");
    let name_text = ui_strings.get_or("UIO_NEWCHAR_STT_NAME", "Name");
    let sex_text = ui_strings.get_or("UIO_NEWCHAR_STT_SEX", "Sex");
    let check_text = ui_strings.get_or("UIO_NEWCHAR_CTL_CHECK_OVERLAP", "Unique check");
    let figure_text = ui_strings.get_or("UIO_NEWCHAR_STT_FIGURE", "Figure");
    let weapon_text = ui_strings.get_or("UIO_NEWCHAR_STT_WEAPON", "Weapon");
    let protector_text = ui_strings.get_or("UIO_NEWCHAR_STT_PROTECTOR", "Cloth");
    let height_text = ui_strings.get_or("UIO_NEWCHAR_STT_HEIGHT", "Height");
    let volume_text = ui_strings.get_or("UIO_NEWCHAR_STT_VOLUME", "Volume");
    let figure_caption_font = fonts.nine.clone();
    let weapon_caption_font = fonts.nine.clone();
    let garment_caption_font = fonts.nine.clone();
    let height_font = fonts.nine.clone();
    let height_value_font = fonts.nine.clone();
    let volume_font = fonts.nine.clone();
    let volume_value_font = fonts.nine.clone();

    let (nlx, nly, nlw, nlh) = NAME_LABEL_RECT;
    let (nex, ney, new_, neh) = NAME_EDIT_RECT;
    let (cbx, cby, cbw, cbh) = CHECK_RECT;
    let (slx, sly, slw, slh) = SEX_LABEL_RECT;
    let (mx, my, mw, mh) = MALE_RECT;
    let (fx, fy, fw, fh) = FEMALE_RECT;
    let (protector_row, weapon_row) = gear_row_tops(race, ROW_GEAR_TOPS);
    let (protector_label, weapon_label) = gear_row_tops(race, LABEL_GEAR_TOPS);

    bsn! {
        CharCreateRoot
        Name("Character Create Panel V2")
        Node {
            position_type: PositionType::Absolute,
            left: percent(3),
            top: percent(22),
            width: px(CUSTOM_W),
            height: px(CUSTOM_H),
        }
        ImageNode { image: {panel_art} }
        TabGroup::new(0)
        Children [
            (
                label(name_text, name_font, 12.0) TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(nlx), top: px(nly), width: px(nlw), height: px(nlh) }
            ),
            // `GDR_EDIT_NAME` has no DDJ of its own: the slot is painted into
            // the panel art, so there is no background node here any more.
            (
                Node { position_type: PositionType::Absolute, left: px(nex), top: px(ney), width: px(new_), height: px(neh) }
                Children [ (text_input(input_font, 0) NameInput) ]
            ),
            (
                image_button(check_style, cbw, cbh)
                ButtonSound({check_sound})
                Node { position_type: PositionType::Absolute, left: px(cbx), top: px(cby), width: px(cbw), height: px(cbh) }
                Children [ (label(check_text, check_font, 11.0) TextColor(Color::WHITE)) ]
                on(on_check_name_activate)
            ),
            (
                label(sex_text, sex_font, 12.0) TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(slx), top: px(sly), width: px(slw), height: px(slh) }
            ),
            (
                image_button(male_style, mw, mh)
                GenderOption(Gender::Male)
                ButtonSound({male_sound})
                Node { position_type: PositionType::Absolute, left: px(mx), top: px(my), width: px(mw), height: px(mh) }
                Children [ (label(male_text, male_font, 11.0) TextColor(Color::WHITE)) ]
                on(|_a: On<Activate>, mut sel: ResMut<CharCreateSelection>| {
                    if sel.gender != Gender::Male {
                        sel.gender = Gender::Male;
                        sel.figure = 0;
                    }
                })
            ),
            (
                image_button(female_style, fw, fh)
                GenderOption(Gender::Female)
                ButtonSound({female_sound})
                Node { position_type: PositionType::Absolute, left: px(fx), top: px(fy), width: px(fw), height: px(fh) }
                Children [ (label(female_text, female_font, 11.0) TextColor(Color::WHITE)) ]
                on(|_a: On<Activate>, mut sel: ResMut<CharCreateSelection>| {
                    if sel.gender != Gender::Female {
                        sel.gender = Gender::Female;
                        sel.figure = 0;
                    }
                })
            ),
            // The five rows, all on the one `Section = Slider` template.
            // Row 1 — `GDR_SLI_FIGURE` (`:352-370`).
            (
                label(figure_text, figure_caption_font, 12.0) TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(LABEL_X), top: px(LABEL_FIGURE_TOP), width: px(LABEL_W), height: px(LABEL_H) }
            ),
            ( slider_row(assets, SliderRow::Figure, ROW_FIGURE_TOP) ),
            // Rows 2 and 3 — `GDR_SLI_HEIGHT` / `_VOLUME`. The slider CHROME is
            // data, but the wire encoding is not: the nibble packing of the
            // scale byte is UNKNOWN and blocked on #241, so the request keeps
            // sending the capture-verified default (see DEFAULT_SCALE). A
            // working-looking slider that cannot be transmitted would be worse
            // than an honest stub, so these two rows stay the dimmed readout.
            (
                label(height_text, height_font, 12.0) TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(LABEL_X), top: px(LABEL_HEIGHT_TOP), width: px(LABEL_W), height: px(LABEL_H) }
            ),
            (
                label("\u{b7} default \u{b7}", height_value_font, 11.0) TextColor(DISABLED_COLOR)
                Node { position_type: PositionType::Absolute, left: px(ROW_X), top: px(ROW_HEIGHT_TOP), width: px(ROW_W), height: px(ROW_H) }
            ),
            (
                label(volume_text, volume_font, 12.0) TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(LABEL_X), top: px(LABEL_VOLUME_TOP), width: px(LABEL_W), height: px(LABEL_H) }
            ),
            (
                label("\u{b7} default \u{b7}", volume_value_font, 11.0) TextColor(DISABLED_COLOR)
                Node { position_type: PositionType::Absolute, left: px(ROW_X), top: px(ROW_VOLUME_TOP), width: px(ROW_W), height: px(ROW_H) }
            ),
            // The two gear rows: `protector_row`/`weapon_row` carry the race's
            // authored order, so CH reads Protector-then-Weapon and EU reads
            // Weapon-then-Protector without a second layout.
            (
                label(protector_text, garment_caption_font, 12.0) TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(LABEL_X), top: px(protector_label), width: px(LABEL_W), height: px(LABEL_H) }
            ),
            ( slider_row(assets, SliderRow::Protector, protector_row) ),
            (
                label(weapon_text, weapon_caption_font, 12.0) TextColor(Color::WHITE)
                Node { position_type: PositionType::Absolute, left: px(LABEL_X), top: px(weapon_label), width: px(LABEL_W), height: px(LABEL_H) }
            ),
            ( slider_row(assets, SliderRow::Weapon, weapon_row) ),
            // Name-check feedback. `GDR_TEXT_MESSAGE` (`:196-214`) is the
            // original's message sink and its rect is `0,0,0,0` — placed by
            // client code, so UNKNOWN. **openroad choice:** the line goes in
            // the panel's free strip below the last row, next to the field it
            // is about, rather than into an invented floating box.
            (
                label("", feedback_font, 11.0) TextColor(Color::WHITE) NameFeedback
                Node { position_type: PositionType::Absolute, left: px(LABEL_X), top: px(FEEDBACK_TOP), width: px(FEEDBACK_W), height: px(LABEL_H) }
            ),
        ]
    }
}

/// One `CIFSliderCtrl` instance at `top`, built from the `Section = Slider`
/// template. Every child rect is the authored one; only which row it drives is
/// ours to say.
fn slider_row(assets: &IntroV2Assets, row: SliderRow, top: f32) -> impl Scene {
    let prev_style = ImageButtonStyle {
        normal: assets.slider_arrow_left.clone(),
        hover: assets.slider_arrow_left_focus.clone(),
        press: assets.slider_arrow_left_press.clone(),
        ..Default::default()
    };
    let next_style = ImageButtonStyle {
        normal: assets.slider_arrow_right.clone(),
        hover: assets.slider_arrow_right_focus.clone(),
        press: assets.slider_arrow_right_press.clone(),
        ..Default::default()
    };
    let thumb_style = ImageButtonStyle {
        normal: assets.slider_thumb.clone(),
        hover: assets.slider_thumb_focus.clone(),
        press: assets.slider_thumb_press.clone(),
        ..Default::default()
    };
    let prev_sound = assets.sound_button_sound_a.clone();
    let next_sound = assets.sound_button_sound_a.clone();
    let (px_, py, pw, ph) = SLIDER_PREV_RECT;
    let (tx, ty, tw, th) = SLIDER_THUMB_RECT;
    let (nx, ny, nw, nh) = SLIDER_NEXT_RECT;

    bsn! {
        Name("Character Create Slider Row")
        Node {
            position_type: PositionType::Absolute,
            left: px(ROW_X),
            top: px(top),
            width: px(SLIDER_TEMPLATE_W),
            height: px(ROW_H),
        }
        Children [
            (
                image_button(prev_style, pw, ph)
                SliderStep(row, {-1})
                ButtonSound({prev_sound})
                Node { position_type: PositionType::Absolute, left: px(px_), top: px(py), width: px(pw), height: px(ph) }
                on(on_slider_step)
            ),
            (
                // The thumb is a `CIFButton` in the data, but dragging it is
                // not something the tree describes; it reads the row's index
                // and is moved by `update_slider_thumbs`.
                ImageNode { image: {thumb_style.normal.clone()}, image_mode: NodeImageMode::Stretch }
                SliderThumb(row)
                Node { position_type: PositionType::Absolute, left: px(tx), top: px(ty), width: px(tw), height: px(th) }
                Pickable::IGNORE
            ),
            (
                image_button(next_style, nw, nh)
                SliderStep(row, 1)
                ButtonSound({next_sound})
                Node { position_type: PositionType::Absolute, left: px(nx), top: px(ny), width: px(nw), height: px(nh) }
                on(on_slider_step)
            ),
        ]
    }
}

/// How many steps a row has and where it currently stands.
fn slider_row_state(
    row: SliderRow,
    selection: &CharCreateSelection,
    char_data: &ClientCharacterData,
    item_index: &ClientItemIndex,
) -> (usize, usize) {
    let (index, count) = match row {
        SliderRow::Figure => (
            selection.figure,
            figure_variants(char_data, selection.race, selection.gender).len(),
        ),
        SliderRow::Protector => (
            Garment::ALL
                .iter()
                .position(|g| *g == selection.garment)
                .unwrap_or(0),
            Garment::ALL.len(),
        ),
        SliderRow::Weapon => (
            selection.weapon,
            weapon_choices(selection.race, item_index).len(),
        ),
    };
    (index.min(count.saturating_sub(1)), count)
}

/// `Activate` observer shared by every slider arrow: steps its row and focuses
/// it, so the Explain box follows the control the user just touched.
pub fn on_slider_step(
    activate: On<Activate>,
    steps: Query<&SliderStep>,
    char_data: Res<ClientCharacterData>,
    item_index: Res<ClientItemIndex>,
    mut focus: ResMut<CharCreateFocus>,
    mut selection: ResMut<CharCreateSelection>,
) {
    let Ok(SliderStep(row, step)) = steps.get(activate.entity) else {
        return;
    };
    let (_, count) = slider_row_state(*row, &selection, &char_data, &item_index);
    if count > 1 {
        let advance = |current: usize| {
            (current as isize + *step as isize).rem_euclid(count as isize) as usize
        };
        match row {
            SliderRow::Figure => selection.figure = advance(selection.figure),
            SliderRow::Weapon => selection.weapon = advance(selection.weapon),
            SliderRow::Protector => {
                selection.garment = selection.garment.cycle(*step as isize);
            }
        }
    }
    if focus.0 != *row {
        focus.0 = *row;
    } else {
        // The row did not change, but its VALUE did — nudge the resource so the
        // Explain box re-reads it.
        focus.set_changed();
    }
}

/// Moves each thumb to its row's current index.
pub fn update_slider_thumbs(
    selection: Res<CharCreateSelection>,
    char_data: Res<ClientCharacterData>,
    item_index: Res<ClientItemIndex>,
    mut thumbs: Query<(&SliderThumb, &mut Node)>,
) {
    for (thumb, mut node) in thumbs.iter_mut() {
        let (index, count) = slider_row_state(thumb.0, &selection, &char_data, &item_index);
        node.left = Val::Px(slider_thumb_left(index, count));
    }
}

/// `GDR_BTN_MALE` / `GDR_BTN_FEMALE`: the original expresses the *selected*
/// state as a texture swap — `man_on.ddj` when male is picked, `man_off.ddj`
/// when it is not — and gives both captions the identical `255,249,212`, so
/// there is no colour cue to read.
fn gender_button_style(assets: &IntroV2Assets, gender: Gender, selected: bool) -> ImageButtonStyle {
    match (gender, selected) {
        (Gender::Male, true) => ImageButtonStyle {
            normal: assets.man_on.clone(),
            hover: assets.man_on_focus.clone(),
            press: assets.man_on_press.clone(),
            ..Default::default()
        },
        (Gender::Male, false) => ImageButtonStyle {
            normal: assets.man_off.clone(),
            hover: assets.man_off_focus.clone(),
            press: assets.man_off_press.clone(),
            ..Default::default()
        },
        (Gender::Female, true) => ImageButtonStyle {
            normal: assets.woman_on.clone(),
            hover: assets.woman_on_focus.clone(),
            press: assets.woman_on_press.clone(),
            ..Default::default()
        },
        (Gender::Female, false) => ImageButtonStyle {
            normal: assets.woman_off.clone(),
            hover: assets.woman_off_focus.clone(),
            press: assets.woman_off_press.clone(),
            ..Default::default()
        },
    }
}

/// The original's preview controls: `GDR_STA_ROTATE`'s 152x56 window with
/// `Section = Rotate`'s three buttons at their authored child rects.
///
/// Idea: the window is a fixed-size sprite, so its children are absolutely
/// placed inside it with the resinfo rects used verbatim — no flex, no
/// padding, nothing to tune. The screen POSITION of the window is the one
/// invented value here: the resinfo rect is `0,0,152,56` like every other
/// panel on this screen, i.e. the original places it from code
/// (`docs/re/ui/scene-intro-character-create.md` §9-U3, UNKNOWN). It is
/// centred under the podium, which is where the preview it drives stands.
fn create_rotate_window(assets: &IntroV2Assets) -> impl Scene {
    let window = assets.rotate_window.clone();
    let left_style = ImageButtonStyle {
        normal: assets.rotate_left.clone(),
        hover: assets.rotate_left_focus.clone(),
        press: assets.rotate_left_press.clone(),
        ..Default::default()
    };
    let right_style = ImageButtonStyle {
        normal: assets.rotate_right.clone(),
        hover: assets.rotate_right_focus.clone(),
        press: assets.rotate_right_press.clone(),
        ..Default::default()
    };
    let zoom_style = zoom_button_style(assets, false);
    let left_sound = assets.sound_button_sound_a.clone();
    let right_sound = assets.sound_button_sound_a.clone();
    let zoom_sound = assets.sound_button_sound_a.clone();
    let (lx, ly, lw, lh) = ROTATE_LEFT_RECT;
    let (zx, zy, zw, zh) = ROTATE_ZOOM_RECT;
    let (rx, ry, rw, rh) = ROTATE_RIGHT_RECT;

    bsn! {
        CharCreateRoot
        Name("Character Create Rotate Window")
        Node {
            position_type: PositionType::Absolute,
            bottom: percent(18),
            left: percent(50),
            width: px(ROTATE_WINDOW_W),
            height: px(ROTATE_WINDOW_H),
            margin: {UiRect::left(Val::Px(-ROTATE_WINDOW_W / 2.0))},
        }
        ImageNode { image: {window} }
        Children [
            (
                image_button(left_style, lw, lh)
                PreviewRotateButton({-PREVIEW_ROTATE_STEP})
                ButtonSound({left_sound})
                Node { position_type: PositionType::Absolute, left: px(lx), top: px(ly), width: px(lw), height: px(lh) }
                on(|activate: On<Activate>,
                    buttons: Query<&PreviewRotateButton>,
                    mut view: ResMut<CharCreatePreviewView>| {
                    if let Ok(button) = buttons.get(activate.entity) {
                        view.yaw += button.0;
                    }
                })
            ),
            (
                image_button(zoom_style, zw, zh)
                PreviewZoomButton
                ButtonSound({zoom_sound})
                Node { position_type: PositionType::Absolute, left: px(zx), top: px(zy), width: px(zw), height: px(zh) }
                on(|_a: On<Activate>, mut view: ResMut<CharCreatePreviewView>| {
                    view.zoomed_in = !view.zoomed_in;
                })
            ),
            (
                image_button(right_style, rw, rh)
                PreviewRotateButton(PREVIEW_ROTATE_STEP)
                ButtonSound({right_sound})
                Node { position_type: PositionType::Absolute, left: px(rx), top: px(ry), width: px(rw), height: px(rh) }
                on(|activate: On<Activate>,
                    buttons: Query<&PreviewRotateButton>,
                    mut view: ResMut<CharCreatePreviewView>| {
                    if let Ok(button) = buttons.get(activate.entity) {
                        view.yaw += button.0;
                    }
                })
            ),
        ]
    }
}

/// `GDR_BTN_ZOOM` is authored with `zoomin.ddj`; `zoomout.ddj` (and both
/// their focus/press variants) ship alongside it, so the zoomed-in state
/// shows the way back out.
fn zoom_button_style(assets: &IntroV2Assets, zoomed_in: bool) -> ImageButtonStyle {
    if zoomed_in {
        ImageButtonStyle {
            normal: assets.zoomout.clone(),
            hover: assets.zoomout_focus.clone(),
            press: assets.zoomout_press.clone(),
            ..Default::default()
        }
    } else {
        ImageButtonStyle {
            normal: assets.zoomin.clone(),
            hover: assets.zoomin_focus.clone(),
            press: assets.zoomin_press.clone(),
            ..Default::default()
        }
    }
}

/// Bottom-right Create / Cancel row, positioned like the char-select controls
/// (original: GDR_BTN_WCREATE / GDR_BTN_WCANCEL).
fn create_controls(
    assets: &IntroV2Assets,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
) -> impl Scene {
    let create_font = fonts.nine.clone();
    let back_font = fonts.nine.clone();
    let create_sound = assets.sound_button_sound_a.clone();
    let back_sound = assets.sound_button_sound_a.clone();
    // `GDR_BTN_OK` (pscharactercreate{china,_europe}.txt:82) carries
    // `UIO_NEWCHAR_CTL_CONFIRM` -> "Confirm". `UIO_COMMON_CTL_CREATE`
    // ("Create") is the *modal's* button, not this one.
    let create_text = ui_strings.get_or("UIO_NEWCHAR_CTL_CONFIRM", "Confirm");
    let back_text = ui_strings.get_or("UIO_COMMON_CTL_CANCEL", "Cancel");

    bsn! {
        CharCreateRoot
        Name("Character Create Controls V2")
        Node {
            position_type: PositionType::Absolute,
            flex_direction: FlexDirection::Row,
            bottom: percent(7.5),
            right: percent(1),
            column_gap: px(15),
        }
        Children [
            (
                image_button(main_button_style(assets), 91.0, 41.0)
                CreateButton
                ButtonSound({create_sound})
                Children [ (label(create_text, create_font, 16.0) TextColor(Color::WHITE)) ]
                on(on_create_activate)
            ),
            (
                image_button(main_button_style(assets), 91.0, 41.0)
                ButtonSound({back_sound})
                Children [ (label(back_text, back_font, 16.0) TextColor(Color::WHITE)) ]
                on(|_a: On<Activate>, mut next_state: ResMut<NextState<IntroV2State>>| {
                    next_state.set(IntroV2State::CharacterList);
                })
            ),
        ]
    }
}

/// `OnEnter(CharacterCreate)`: clears the keep-alive marker and spawns the
/// creation UI; the preview is spawned by [`update_preview`] on the first
/// change of the freshly-inserted [`CharCreateSelection`].
pub fn enter_character_create(
    mut commands: Commands,
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    asset_server: Res<AssetServer>,
    selection: Option<Res<CharCreateSelection>>,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    commands.remove_resource::<EnteringCharacterCreate>();
    // The race is picked on the region-select board, which inserts the
    // resource before the state switch; `init_resource` keeps that choice and
    // only fills in a default when the screen is entered some other way.
    let race = selection.map(|s| s.race).unwrap_or_default();
    commands.init_resource::<CharCreateSelection>();
    commands.init_resource::<CharCreatePreviewView>();
    commands.init_resource::<CharCreateFocus>();

    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found");
        return;
    };
    commands
        .spawn_scene(create_panel(&assets, &fonts, &ui_strings, race))
        .insert((UiTargetCamera(camera), IntroV2Ui));
    commands
        .spawn_scene(create_controls(&assets, &fonts, &ui_strings))
        .insert((UiTargetCamera(camera), IntroV2Ui));
    commands
        .spawn_scene(create_rotate_window(&assets))
        .insert((UiTargetCamera(camera), IntroV2Ui));

    // `GDR_STA_TITLE` (:215-233): a baked 428x36 image at 47,111 in the
    // 1600x1200 design space — the same shape region-select already uses for
    // `text-region.ddj`, so the position is expressed as the design-space
    // fraction and the art keeps its native size.
    commands.spawn((
        CharCreateRoot,
        IntroV2Ui,
        UiTargetCamera(camera),
        Name::from("Character Create Title"),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(47.0 / 16.0),
            top: Val::Percent(111.0 / 12.0),
            width: Val::Px(TITLE_ART_W),
            height: Val::Px(TITLE_ART_H),
            ..default()
        },
        ImageNode::new(asset_server.load(TITLE_CUSTOM_DDJ)),
        Pickable::IGNORE,
    ));

    // `GDR_STA_EXPLAIN` + `Section = Explain`: the figure's vanilla name and
    // backstory on the screen's own panel art, replacing the hand-built box
    // that borrowed the char-select chrome. The European tree swaps both the
    // art and the size (see EXPLAIN_EU_* for the one rect/art mismatch).
    let (explain_art, (explain_w, explain_h)) = match race {
        Race::Chinese => (assets.explain_window.clone(), EXPLAIN_CH_SIZE),
        Race::European => (assets.explain_window_02.clone(), EXPLAIN_EU_ART),
    };
    commands
        .spawn((
            CharCreateRoot,
            IntroV2Ui,
            UiTargetCamera(camera),
            Name::from("Character Create Story V2"),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Percent(3.0),
                top: Val::Percent(22.0),
                width: Val::Px(explain_w),
                height: Val::Px(explain_h),
                ..default()
            },
            ImageNode::new(explain_art),
            Pickable::IGNORE,
        ))
        .with_children(|panel| {
            panel.spawn((
                FigureStoryTitle,
                Text::new(""),
                TextFont {
                    font: fonts.nine.clone().into(),
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                TextColor(EXPLAIN_NAME_COLOR),
                explain_child_node(EXPLAIN_NAME_RECT),
                Pickable::IGNORE,
            ));
            panel.spawn((
                FigureStoryText,
                Text::new(""),
                TextFont {
                    font: fonts.nine.clone().into(),
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(Color::WHITE),
                explain_child_node(EXPLAIN_BODY_RECT),
                Pickable::IGNORE,
            ));
        });
}

/// A `Section = Explain` child at its authored rect, in the panel's own space.
fn explain_child_node(rect: (f32, f32, f32, f32)) -> Node {
    let (left, top, width, height) = rect;
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(left),
        top: Val::Px(top),
        width: Val::Px(width),
        height: Val::Px(height),
        ..default()
    }
}

/// `OnEnter(CharacterCreate)`: snaps the stage camera to the scene data's held
/// creation pose (`create_camera_transforms`) instead of replaying the
/// char-select fly-in. A single keyframe cannot form a tween, so a pose-set is
/// the correct mechanism; an empty keyframe list falls back to the lineup's
/// end pose.
pub fn set_create_camera_pose(
    mut cam_query: Query<&mut Transform, With<CinematicCamera2>>,
    scene: Res<ActiveCharSelectSceneV2>,
    origin: Res<WorldOrigin>,
) {
    let Ok(mut transform) = cam_query.single_mut() else {
        return;
    };
    let Some((translation, rotation)) = create_camera_pose(&scene, &origin) else {
        return;
    };
    transform.translation = translation;
    transform.rotation = rotation;
}

/// The held creation pose, with the lineup's end pose as the fallback.
fn create_camera_pose(
    scene: &ActiveCharSelectSceneV2,
    origin: &WorldOrigin,
) -> Option<(Vec3, Quat)> {
    scene
        .0
        .create_camera_pose(origin.0)
        .or_else(|| scene.0.init_camera_end_pose(origin.0))
}

/// Applies `Section = Rotate`'s state: the accumulated yaw turns the preview
/// on the podium, and the zoom toggle dollies the create camera along its own
/// view axis (so the pose the scene data holds stays the anchor — the zoom is
/// an offset from it, never a second source of truth). The zoom button's own
/// art swaps `zoomin` <-> `zoomout` with the state.
pub fn apply_preview_view(
    view: Res<CharCreatePreviewView>,
    assets: Res<IntroV2Assets>,
    scene: Res<ActiveCharSelectSceneV2>,
    origin: Res<WorldOrigin>,
    mut preview: Query<&mut Transform, With<CharCreatePreview>>,
    mut camera: Query<&mut Transform, (With<CinematicCamera2>, Without<CharCreatePreview>)>,
    mut zoom_button: Query<(&mut ImageNode, &mut ImageButtonStyle), With<PreviewZoomButton>>,
) {
    for mut transform in preview.iter_mut() {
        transform.rotation = preview_rotation(view.yaw);
    }
    if let (Ok(mut transform), Some((translation, rotation))) =
        (camera.single_mut(), create_camera_pose(&scene, &origin))
    {
        let dolly = if view.zoomed_in {
            PREVIEW_ZOOM_DOLLY
        } else {
            0.0
        };
        transform.translation = translation + rotation * (Vec3::NEG_Z * dolly);
        transform.rotation = rotation;
    }
    for (mut image, mut style) in zoom_button.iter_mut() {
        *style = zoom_button_style(&assets, view.zoomed_in);
        image.image = style.normal.clone();
    }
}

/// The podium pose (`PI`, facing the camera) plus the user's accumulated yaw.
fn preview_rotation(yaw: f32) -> Quat {
    Quat::from_rotation_y(PI + yaw)
}

/// Re-spawns the 3d preview whenever the selection changes (and once on enter,
/// via the resource-added change). Centers a single body like a one-entry
/// char-select lineup, reusing the same mirroring + equipment path.
pub fn update_preview(
    selection: Res<CharCreateSelection>,
    view: Res<CharCreatePreviewView>,
    scene: Res<ActiveCharSelectSceneV2>,
    char_data: Res<ClientCharacterData>,
    item_data: Res<ClientItemData>,
    item_index: Res<ClientItemIndex>,
    asset_server: Res<AssetServer>,
    origin: Res<WorldOrigin>,
    existing: Query<Entity, With<CharCreatePreview>>,
    mut feedback: Query<&mut Text, With<NameFeedback>>,
    mut commands: Commands,
) {
    // Resolve before despawning: an unresolvable selection (e.g. European on
    // a corpus without EU rows) keeps the previous preview on stage and
    // reports the gap in-UI instead of silently emptying the podium.
    let Some(refs) = resolve_starter(&selection, &char_data, &item_index) else {
        warn!(
            "starter data unavailable for {:?}/{:?}",
            selection.race, selection.gender
        );
        if let Ok(mut text) = feedback.single_mut() {
            text.0 = "Starter data unavailable for this selection.".to_string();
        }
        return;
    };

    for entity in existing.iter() {
        commands.entity(entity).despawn();
    }
    let Some(path) = char_data
        .get(&(refs.body as i32))
        .and_then(|row| char_data.model_path(row))
    else {
        return;
    };

    let begin = scene.0.char_start_offset() * Vec3::new(-1.0, 1.0, 1.0);
    let end = scene.0.char_end_offset() * Vec3::new(-1.0, 1.0, 1.0);
    let base = origin.to_render(scene.0.cam_base() * Vec3::new(-1.0, 1.0, 1.0)) + begin;
    let pos = base + (end - begin) / 2.0;
    let scale = 1.0 + DEFAULT_SCALE as f32 / 255.0;
    // The re-spawn keeps whatever the rotate buttons have set, so changing
    // gender or gear does not snap the body back to the podium pose.
    let transform = Transform::from_translation(pos)
        .with_scale(Vec3::new(-scale, scale, scale))
        .with_rotation(preview_rotation(view.yaw));

    let mut root = commands.spawn((
        transform,
        Visibility::default(),
        CharCreatePreview,
        UnloadedResource(asset_server.load(path)),
        Name::from("Character Create Preview"),
    ));
    if needs_winding_reversal(&transform.to_matrix()) {
        root.insert(MirroredResource);
    }
    let char_entity = root.id();
    character_select::attach_equipment(
        &mut commands,
        &asset_server,
        &item_data,
        char_entity,
        [refs.chest, refs.pants, refs.boots, refs.weapon]
            .into_iter()
            .map(|id| (id, None)),
    );
}

/// Swaps the gender toggles' textures so the picked one reads `*_on` — the
/// original's own selection cue (both captions carry the same `255,249,212`,
/// so a colour change would be our invention). Race is fixed by the
/// region-select board and has no toggle here.
pub fn highlight_selection_buttons(
    selection: Res<CharCreateSelection>,
    assets: Res<IntroV2Assets>,
    mut genders: Query<(&GenderOption, &mut ImageNode, &mut ImageButtonStyle)>,
) {
    for (option, mut image, mut style) in genders.iter_mut() {
        *style = gender_button_style(&assets, option.0, option.0 == selection.gender);
        image.image = style.normal.clone();
    }
}

/// Fills the `Section = Explain` panel from the focused row.
///
/// Idea: the original's rows carry no value text — `GDR_TEXT_EXPLAIN` is the
/// readout, and it is context-sensitive per focused slider, which is why 24
/// `UIO_NEWCHAR_EXPLANATION_*` keys ship for the armour types, the weapon types
/// and Height/Volume. The key scheme is mechanical: a row's caption key
/// `UIO_NEWCHAR_STT_<X>` has the description `UIO_NEWCHAR_EXPLANATION_<X>`
/// (textuisystem:322-341, 394-417); only the figure rows use the older
/// `<key>_EXPLANATION` suffix form.
pub fn update_explain_panel(
    selection: Res<CharCreateSelection>,
    focus: Res<CharCreateFocus>,
    char_data: Res<ClientCharacterData>,
    item_index: Res<ClientItemIndex>,
    ui_strings: Res<ClientUiStrings>,
    mut texts: ParamSet<(
        Query<&mut Text, With<FigureStoryTitle>>,
        Query<&mut Text, With<FigureStoryText>>,
    )>,
) {
    // Two inputs, so the change check is in the body rather than a run
    // condition: either a new value or a new focused row re-reads the box.
    if !selection.is_changed() && !focus.is_changed() {
        return;
    }
    let (name, body) = match focus.0 {
        SliderRow::Figure => {
            let figures = figure_variants(&char_data, selection.race, selection.gender);
            let index = selection.figure.min(figures.len().saturating_sub(1));
            match figures.get(index) {
                Some((_, code_name)) => {
                    let key = figure_label_key(selection.race, selection.gender, code_name);
                    let name = ui_strings
                        .get_or(&key, &humanize_codename(code_name))
                        .to_string();
                    let body = ui_strings
                        .get_or(&format!("{key}_EXPLANATION"), "")
                        .to_string();
                    (name, body)
                }
                None => ("—".to_string(), String::new()),
            }
        }
        SliderRow::Protector => {
            let (key, fallback) = selection.garment.label(selection.race);
            (
                ui_strings.get_or(key, fallback).to_string(),
                ui_strings.get_or(&explanation_key(key), "").to_string(),
            )
        }
        SliderRow::Weapon => {
            let weapons = weapon_choices(selection.race, &item_index);
            match weapons.get(selection.weapon.min(weapons.len().saturating_sub(1))) {
                Some((_, key, fallback)) => (
                    ui_strings.get_or(key, fallback).to_string(),
                    ui_strings.get_or(&explanation_key(key), "").to_string(),
                ),
                None => ("—".to_string(), String::new()),
            }
        }
    };
    if let Ok(mut text) = texts.p0().single_mut() {
        text.0 = name;
    }
    if let Ok(mut text) = texts.p1().single_mut() {
        // textdata carries the line breaks as the two characters `\` `n`.
        text.0 = body.replace("\\n", "\n");
    }
}

/// `UIO_NEWCHAR_STT_<X>` -> `UIO_NEWCHAR_EXPLANATION_<X>`; a key that does not
/// follow the scheme is returned unchanged and simply resolves to nothing.
fn explanation_key(caption_key: &str) -> String {
    caption_key.replace("_STT_", "_EXPLANATION_")
}

/// `CHAR_CH_MAN_ADVENTURER` -> "Adventurer", for corpora without the
/// textuisystem rows.
fn humanize_codename(code_name: &str) -> String {
    code_name
        .rsplit('_')
        .next()
        .map(|s| {
            let mut chars = s.chars();
            chars
                .next()
                .map(|f| f.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// `Activate` observer of the Check button: sends a `CheckName` probe for the
/// current name; [`on_check_name_response`] shows the outcome.
pub fn on_check_name_activate(
    _activate: On<Activate>,
    name_query: Query<&EditableText, With<NameInput>>,
    conn_query: Query<&SilkroadConnection, With<AgentConnection>>,
    ui_strings: Res<ClientUiStrings>,
    mut feedback: Query<&mut Text, With<NameFeedback>>,
) {
    let name = current_name(&name_query);
    let Ok(mut text) = feedback.single_mut() else {
        return;
    };
    if let Err(msg) = validate_name(&name, &ui_strings) {
        text.0 = msg;
        return;
    }
    let Ok(conn) = conn_query.single() else {
        return;
    };
    send_action(
        conn,
        CharacterSelectionActionRequest {
            action: CharacterSelectionAction::CheckName,
            name: Some(name),
            create: None,
        },
    );
    text.0 = "Checking name...".to_string();
}

/// Shows the `CheckName` availability result (original messages: "Valid ID" /
/// "This ID already exists.").
pub fn on_check_name_response(
    mut reader: MessageReader<CharacterSelectionActionResponse>,
    ui_strings: Res<ClientUiStrings>,
    mut feedback: Query<&mut Text, With<NameFeedback>>,
) {
    for res in reader.read() {
        if res.action != CharacterSelectionAction::CheckName {
            continue;
        }
        let Ok(mut text) = feedback.single_mut() else {
            continue;
        };
        text.0 = if res.result == 1 {
            ui_strings.get_plain_or("UIO_MSG_ERROR_ADMISSON", "Valid ID")
        } else {
            ui_strings.get_plain_or("UIO_MSG_ERROR_ID", "This ID already exists.")
        };
    }
}

/// The `Section = WCreate` confirmation modal: the name about to be created,
/// the original's own confirmation question, and the two 76x32 buttons.
///
/// Idea: this is char-select's delete-confirm shape, reused rather than
/// re-derived — a full-screen scrim that eats the clicks behind it, carrying
/// the warning frame at its native size. The frame's own rect is `0,0,248,128`,
/// i.e. code-placed in the original (UNKNOWN), so centring it is our choice and
/// matches the sibling modal already on screen one step earlier.
fn create_confirm_modal(
    character_name: &str,
    assets: &IntroV2Assets,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
) -> impl Scene {
    let window = assets.warning_create_window.clone();
    let name_font = fonts.nine.clone();
    let msg_font = fonts.nine.clone();
    let ok_font = fonts.nine.clone();
    let cancel_font = fonts.nine.clone();
    let ok_sound = assets.sound_button_sound_a.clone();
    let cancel_sound = assets.sound_button_sound_a.clone();
    let name_text = character_name.to_string();
    let message = ui_strings
        .get_or(
            "UIO_NEWCHAR_MSG_CREATE",
            "Do you want to create a new character?",
        )
        .replace("\\n", "\n");
    let ok_text = ui_strings.get_or("UIO_COMMON_CTL_CREATE", "Create");
    let cancel_text = ui_strings.get_or("UIO_COMMON_CTL_CANCEL", "Cancel");
    let (nx, ny, nw, nh) = WCREATE_NAME_RECT;
    let (mx, my, mw, mh) = WCREATE_MSG_RECT;
    let (ox, oy, ow, oh) = WCREATE_OK_RECT;
    let (cx, cy, cw, chh) = WCREATE_CANCEL_RECT;

    bsn! {
        CreateConfirmModal
        CharCreateRoot
        Name("Character Create Confirm Modal")
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
        }
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75))
        Children [
            (
                ImageNode { image: {window}, image_mode: NodeImageMode::Stretch }
                Node {
                    position_type: PositionType::Absolute,
                    width: px(WCREATE_W),
                    height: px(WCREATE_H),
                    margin: {UiRect::all(Val::Auto)},
                }
                Children [
                    (
                        // label() starts transparent for the intro fade
                        // systems, which never run on this modal.
                        label(&name_text, name_font, 12.0) TextColor(Color::WHITE)
                        Node { position_type: PositionType::Absolute, left: px(nx), top: px(ny), width: px(nw), height: px(nh) }
                    ),
                    (
                        label(&message, msg_font, 11.0) TextColor(Color::WHITE)
                        Node { position_type: PositionType::Absolute, left: px(mx), top: px(my), width: px(mw), height: px(mh) }
                    ),
                    (
                        image_button(character_select::warning_button_style(assets), ow, oh)
                        ButtonSound({ok_sound})
                        Node { position_type: PositionType::Absolute, left: px(ox), top: px(oy), width: px(ow), height: px(oh) }
                        Children [ (label(ok_text, ok_font, 11.0) TextColor(Color::WHITE)) ]
                        on(on_create_confirm_activate)
                    ),
                    (
                        image_button(character_select::warning_button_style(assets), cw, chh)
                        ButtonSound({cancel_sound})
                        Node { position_type: PositionType::Absolute, left: px(cx), top: px(cy), width: px(cw), height: px(chh) }
                        Children [ (label(cancel_text, cancel_font, 11.0) TextColor(Color::WHITE)) ]
                        on(|_a: On<Activate>,
                            modal_query: Query<Entity, With<CreateConfirmModal>>,
                            mut commands: Commands| {
                            for modal in modal_query.iter() {
                                commands.entity(modal).despawn();
                            }
                        })
                    ),
                ]
            ),
        ]
    }
}

/// `Activate` observer of the screen's Confirm button. It no longer sends
/// anything: the original asks `UIO_NEWCHAR_MSG_CREATE` first, so this
/// validates the name, proves the starter set resolves, and opens the modal.
/// The request itself leaves from [`on_create_confirm_activate`].
pub fn on_create_activate(
    _activate: On<Activate>,
    selection: Res<CharCreateSelection>,
    name_query: Query<&EditableText, With<NameInput>>,
    char_data: Res<ClientCharacterData>,
    item_index: Res<ClientItemIndex>,
    existing_modal: Query<(), With<CreateConfirmModal>>,
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    cam_query: Query<Entity, With<Camera2d>>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
    mut commands: Commands,
) {
    if !existing_modal.is_empty() {
        return;
    }
    let name = current_name(&name_query);
    if let Err(msg) = validate_name(&name, &ui_strings) {
        info_text_writer.write(InfoTextV2Update(msg));
        return;
    }
    if resolve_starter(&selection, &char_data, &item_index).is_none() {
        info_text_writer.write(InfoTextV2Update(
            "Starter data unavailable for this selection.".to_string(),
        ));
        return;
    }
    let Some(camera) = cam_query.iter().next() else {
        return;
    };
    commands
        .spawn_scene(create_confirm_modal(&name, &assets, &fonts, &ui_strings))
        .insert((UiTargetCamera(camera), IntroV2Ui));
}

/// `Activate` observer of the modal's Create button: the only place the
/// `Create` request leaves the client. [`on_character_create_response`] handles
/// the outcome and closes the modal.
pub fn on_create_confirm_activate(
    activate: On<Activate>,
    selection: Res<CharCreateSelection>,
    name_query: Query<&EditableText, With<NameInput>>,
    char_data: Res<ClientCharacterData>,
    item_index: Res<ClientItemIndex>,
    conn_query: Query<&SilkroadConnection, With<AgentConnection>>,
    mut commands: Commands,
) {
    let name = current_name(&name_query);
    let Some(refs) = resolve_starter(&selection, &char_data, &item_index) else {
        return;
    };
    let Ok(conn) = conn_query.single() else {
        return;
    };

    // double-activation guard (same pattern as the Start button); the response
    // re-enables it on rejection
    commands
        .entity(activate.entity)
        .insert(InteractionDisabled)
        .remove::<Pressed>();

    let sent = send_action(
        conn,
        CharacterSelectionActionRequest {
            action: CharacterSelectionAction::Create,
            name: None,
            create: Some(CharacterCreate {
                name,
                ref_obj_id: refs.body,
                scale: DEFAULT_SCALE,
                chest: refs.chest,
                pants: refs.pants,
                boots: refs.boots,
                weapon: refs.weapon,
            }),
        },
    );
    if !sent {
        commands
            .entity(activate.entity)
            .remove::<InteractionDisabled>();
    }
}

/// Handles the create response: success returns to a refreshed `CharacterList`;
/// a server rejection (e.g. the same server gap seen on item-use) shows the
/// info text + error sound and keeps the screen so no panic/disconnect occurs.
pub fn on_character_create_response(
    mut reader: MessageReader<CharacterSelectionActionResponse>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
    mut next_state: ResMut<NextState<IntroV2State>>,
    create_buttons: Query<Entity, (With<CreateButton>, With<InteractionDisabled>)>,
    modal_query: Query<Entity, With<CreateConfirmModal>>,
    assets: Res<IntroV2Assets>,
    ui_strings: Res<ClientUiStrings>,
    options: Res<GameOptions>,
    mut commands: Commands,
) {
    for res in reader.read() {
        if res.action != CharacterSelectionAction::Create {
            continue;
        }
        if res.result == 1 {
            next_state.set(IntroV2State::CharacterList);
        } else {
            // the per-code error catalogue is an open R6 item; the code goes
            // to the log, the UI shows the original generic failure text
            warn!(
                "character create rejected (error {})",
                res.error_code.unwrap_or_default()
            );
            info_text_writer.write(InfoTextV2Update(
                ui_strings
                    .get_or(
                        "UIO_SMERR_FAILED_TO_CREATE_CHARACTER",
                        "Failed to create a character. Please try to connect again.",
                    )
                    .to_string(),
            ));
            for entity in create_buttons.iter() {
                commands.entity(entity).remove::<InteractionDisabled>();
            }
            // The request left from the modal, so the modal is what has to go:
            // leaving a disabled Create button behind a scrim would strand the
            // screen. Closing it puts the user back on the panel to fix the
            // name or the selection and ask again.
            for modal in modal_query.iter() {
                commands.entity(modal).despawn();
            }
            if let Some(playback) = options.audio.fx_playback() {
                commands.spawn((AudioPlayer::new(assets.sound_error.clone()), playback));
            }
        }
    }
}

/// `OnExit(CharacterCreate)`: tears the UI + preview down and drops the
/// selection (the camera is despawned by the shared `despawn_cinematic_camera`).
pub fn despawn_character_create(
    ui: Query<Entity, With<CharCreateRoot>>,
    preview: Query<Entity, With<CharCreatePreview>>,
    mut commands: Commands,
) {
    for entity in ui.iter().chain(preview.iter()) {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<CharCreateSelection>();
    commands.remove_resource::<CharCreatePreviewView>();
    commands.remove_resource::<CharCreateFocus>();
}

fn current_name(name_query: &Query<&EditableText, With<NameInput>>) -> String {
    name_query
        .single()
        .map(|input| input.value().to_string().trim().to_string())
        .unwrap_or_default()
}

/// Pre-send validation mirroring the original client: non-empty, at most
/// [`MAX_NAME_LEN`] characters, no inner whitespace. Returns the original
/// error message on violation.
fn validate_name(name: &str, ui_strings: &ClientUiStrings) -> Result<(), String> {
    if name.is_empty() {
        return Err(ui_strings.get_plain_or(
            "UIO_MSG_ERROR_CHARACTER_NAME",
            "Enter the name of character.",
        ));
    }
    if name.chars().count() > MAX_NAME_LEN || name.contains(char::is_whitespace) {
        return Err(ui_strings.get_plain_or(
            "UIO_MSG_ERROR_CHARACTER_NAME_STRING",
            "Exceeded the letter limit. Only 12 English letters are available.",
        ));
    }
    Ok(())
}

/// Sends a lobby action frame; returns whether it was queued.
fn send_action(conn: &SilkroadConnection, request: CharacterSelectionActionRequest) -> bool {
    let frame = Packet::from(request).into();
    if let Err(e) = conn.get_sender().send(frame) {
        error!("failed to send frame: {}", e.0);
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Section = Slider` is ONE authored template, transcribed verbatim, and
    /// its drawn width is the next arrow's right edge — 140, not the row
    /// `Rect`'s 120. The panel is what makes that credible: the rows start at
    /// x=88 and 88+140 still fits inside the 264-wide window.
    #[test]
    fn the_slider_template_is_the_authored_one_and_fits_the_panel() {
        assert_eq!(SLIDER_PREV_RECT, (0.0, 2.0, 20.0, 20.0));
        assert_eq!(SLIDER_THUMB_RECT, (20.0, 0.0, 16.0, 24.0));
        assert_eq!(SLIDER_NEXT_RECT, (120.0, 2.0, 20.0, 20.0));
        assert_eq!(SLIDER_TEMPLATE_W, 140.0);
        assert!(ROW_X + SLIDER_TEMPLATE_W <= CUSTOM_W);
        // the template is taller than the arrows: the thumb defines the height
        assert_eq!(SLIDER_THUMB_RECT.3, ROW_H);
    }

    /// The thumb's travel is derived from the authored rects, not chosen: it
    /// starts at the thumb's own x, ends where the next arrow begins, and the
    /// steps divide that evenly.
    #[test]
    fn the_thumb_spans_the_authored_track_and_never_leaves_it() {
        let start = SLIDER_THUMB_RECT.0;
        let track_end = SLIDER_NEXT_RECT.0;
        for count in [2usize, 3, 5, 26] {
            assert_eq!(slider_thumb_left(0, count), start, "count {count}");
            assert_eq!(
                slider_thumb_left(count - 1, count),
                track_end - SLIDER_THUMB_RECT.2,
                "count {count}"
            );
            let mut previous = f32::MIN;
            for index in 0..count {
                let left = slider_thumb_left(index, count);
                assert!(left > previous, "thumb must advance at {index}/{count}");
                assert!(left >= start && left + SLIDER_THUMB_RECT.2 <= track_end);
                previous = left;
            }
        }
        // a row with nothing (or one thing) to choose parks at the start
        assert_eq!(slider_thumb_left(0, 0), start);
        assert_eq!(slider_thumb_left(0, 1), start);
        // an out-of-range index is clamped, never drawn past the track
        assert_eq!(slider_thumb_left(99, 3), slider_thumb_left(2, 3));
    }

    /// The Explain box is the rows' readout, and its key scheme is mechanical:
    /// the caption key's `_STT_` becomes `_EXPLANATION_` (textuisystem:322-341,
    /// 394-417). Pinned against the real keys of both race trees.
    #[test]
    fn the_explanation_key_is_the_caption_key_with_one_segment_swapped() {
        assert_eq!(
            explanation_key("UIO_NEWCHAR_STT_LIGHT_ARMOR"),
            "UIO_NEWCHAR_EXPLANATION_LIGHT_ARMOR"
        );
        assert_eq!(
            explanation_key("UIO_NEWCHAR_STT_EU_ROBE"),
            "UIO_NEWCHAR_EXPLANATION_EU_ROBE"
        );
        assert_eq!(
            explanation_key("UIO_NEWCHAR_STT_TWOHANDSTAFF"),
            "UIO_NEWCHAR_EXPLANATION_TWOHANDSTAFF"
        );
        // every garment caption of both races maps onto a shipped key name
        for race in [Race::Chinese, Race::European] {
            for set in Garment::ALL {
                let (key, _) = set.label(race);
                assert!(explanation_key(key).starts_with("UIO_NEWCHAR_EXPLANATION_"));
            }
        }
        // a key outside the scheme is left alone rather than mangled
        assert_eq!(
            explanation_key("UIO_COMMON_CTL_CANCEL"),
            "UIO_COMMON_CTL_CANCEL"
        );
    }

    /// `Section = WCreate` is a complete authored modal and is transcribed
    /// verbatim: the two 76x32 buttons, the name line and the message line.
    /// The arithmetic closes inside the 248x128 frame, so nothing here is a
    /// magic number: 4+239 = 243 < 248, and 130+76 = 206 < 248.
    #[test]
    fn the_create_confirm_modal_uses_the_authored_wcreate_rects() {
        assert_eq!((WCREATE_W, WCREATE_H), (248.0, 128.0));
        assert_eq!(WCREATE_NAME_RECT, (4.0, 23.0, 239.0, 15.0));
        assert_eq!(WCREATE_MSG_RECT, (4.0, 47.0, 239.0, 13.0));
        assert_eq!(WCREATE_OK_RECT, (42.0, 75.0, 76.0, 32.0));
        assert_eq!(WCREATE_CANCEL_RECT, (130.0, 75.0, 76.0, 32.0));
        // every child stays inside the frame
        for (x, y, w, h) in [
            WCREATE_NAME_RECT,
            WCREATE_MSG_RECT,
            WCREATE_OK_RECT,
            WCREATE_CANCEL_RECT,
        ] {
            assert!(x + w <= WCREATE_W, "{x}+{w} escapes the frame");
            assert!(y + h <= WCREATE_H, "{y}+{h} escapes the frame");
        }
        // the two buttons share a row and do not overlap
        assert_eq!(WCREATE_OK_RECT.1, WCREATE_CANCEL_RECT.1);
        assert!(WCREATE_OK_RECT.0 + WCREATE_OK_RECT.2 <= WCREATE_CANCEL_RECT.0);
    }

    /// The customize panel and every `Section = Custom` child are transcribed
    /// from the resinfo tree, not laid out by flex/padding. If any of these
    /// drift, the panel art and its slots stop lining up.
    #[test]
    fn the_customize_panel_children_are_the_authored_resinfo_rects() {
        assert_eq!((CUSTOM_W, CUSTOM_H), (264.0, 316.0));
        assert_eq!(NAME_LABEL_RECT, (32.0, 35.0, 45.0, 15.0));
        assert_eq!(NAME_EDIT_RECT, (92.0, 32.0, 135.0, 20.0));
        assert_eq!(CHECK_RECT, (156.0, 57.0, 75.0, 25.0));
        assert_eq!(SEX_LABEL_RECT, (32.0, 100.0, 45.0, 15.0));
        assert_eq!(MALE_RECT, (87.0, 95.0, 70.0, 25.0));
        assert_eq!(FEMALE_RECT, (166.0, 95.0, 70.0, 25.0));
        // Five rows, one pitch: 131/162/193/224/255, step 31.
        let rows = [
            ROW_FIGURE_TOP,
            ROW_HEIGHT_TOP,
            ROW_VOLUME_TOP,
            ROW_GEAR_TOPS.0,
            ROW_GEAR_TOPS.1,
        ];
        assert_eq!(rows, [131.0, 162.0, 193.0, 224.0, 255.0]);
        for pair in rows.windows(2) {
            assert_eq!(pair[1] - pair[0], 31.0);
        }
        assert_eq!((ROW_X, ROW_W, ROW_H), (88.0, 120.0, 24.0));
        assert_eq!((LABEL_X, LABEL_W, LABEL_H), (32.0, 45.0, 15.0));
        assert_eq!(
            [
                LABEL_FIGURE_TOP,
                LABEL_HEIGHT_TOP,
                LABEL_VOLUME_TOP,
                LABEL_GEAR_TOPS.0,
                LABEL_GEAR_TOPS.1
            ],
            [137.0, 167.0, 198.0, 229.0, 260.0]
        );
    }

    /// The CH/EU gear-row swap is behavioural, not decoration: Chinese reads
    /// Protector-then-Weapon (`china :295-313` / `:276-294`), European reads
    /// Weapon-then-Protector. Two layouts, never an average of them.
    #[test]
    fn the_gear_rows_swap_between_the_race_trees() {
        assert_eq!(gear_row_tops(Race::Chinese, ROW_GEAR_TOPS), (224.0, 255.0));
        assert_eq!(gear_row_tops(Race::European, ROW_GEAR_TOPS), (255.0, 224.0));
        assert_eq!(
            gear_row_tops(Race::Chinese, LABEL_GEAR_TOPS),
            (229.0, 260.0)
        );
        assert_eq!(
            gear_row_tops(Race::European, LABEL_GEAR_TOPS),
            (260.0, 229.0)
        );
        // and the swap is a swap, not a shift: the pair of occupied rows is
        // the same set for both races.
        let (cp, cw) = gear_row_tops(Race::Chinese, ROW_GEAR_TOPS);
        let (ep, ew) = gear_row_tops(Race::European, ROW_GEAR_TOPS);
        assert_eq!((cp.min(cw), cp.max(cw)), (ep.min(ew), ep.max(ew)));
    }

    /// The Explain panel's children are authored; its European art is the
    /// tree's one rect/art mismatch and is kept visible rather than lost.
    #[test]
    fn the_explain_panel_records_the_eu_rect_art_mismatch() {
        assert_eq!(EXPLAIN_NAME_RECT, (15.0, 18.0, 183.0, 14.0));
        assert_eq!(EXPLAIN_BODY_RECT, (15.0, 42.0, 183.0, 122.0));
        assert_eq!(EXPLAIN_CH_SIZE, (212.0, 180.0));
        // The EU tree declares 212x250 but ships a 220x236 sprite; we draw the
        // art at its own size, and this pins that the two genuinely differ.
        assert_ne!(EXPLAIN_EU_RECT, EXPLAIN_EU_ART);
        assert_eq!(EXPLAIN_EU_ART, (220.0, 236.0));
    }

    /// The protector row is one cycling control (the original is one slider),
    /// so stepping through it wraps and returns to where it started.
    #[test]
    fn the_protector_row_cycles_through_all_three_sets() {
        let mut set = Garment::Clothes;
        for _ in 0..Garment::ALL.len() {
            set = set.cycle(1);
        }
        assert_eq!(set, Garment::Clothes);
        assert_eq!(Garment::Clothes.cycle(-1), Garment::Heavy);
        assert_eq!(Garment::Clothes.cycle(1), Garment::Light);
    }

    /// `GDR_STA_TITLE` (`pscharactercreate{china,_europe}.txt:215`) is a baked
    /// 428x36 image at 47,111 — `text-custom.ddj`, shared by both race trees.
    /// We drew a hardcoded "Create Character" string with no textuisystem key.
    #[test]
    fn the_title_is_baked_art_at_its_resinfo_size() {
        assert_eq!((TITLE_ART_W, TITLE_ART_H), (428.0, 36.0));
        assert!(TITLE_CUSTOM_DDJ.ends_with("outer/text-custom.ddj"));
    }

    /// `Section = Rotate` is a complete authored template and is transcribed
    /// verbatim: the 152x56 window (`GDR_STA_ROTATE`, :120-138) and the three
    /// child rects (`:626-682`). Nothing here is padding or taste.
    #[test]
    fn the_rotate_window_and_its_buttons_use_the_authored_resinfo_rects() {
        assert_eq!((ROTATE_WINDOW_W, ROTATE_WINDOW_H), (152.0, 56.0));
        assert_eq!(ROTATE_LEFT_RECT, (12.0, 11.0, 42.0, 35.0));
        assert_eq!(ROTATE_ZOOM_RECT, (57.0, 11.0, 39.0, 35.0));
        assert_eq!(ROTATE_RIGHT_RECT, (99.0, 11.0, 42.0, 35.0));
        // the two rotate buttons are the same size and sit symmetrically
        // about the zoom button's centre seam
        assert_eq!(ROTATE_LEFT_RECT.2, ROTATE_RIGHT_RECT.2);
        assert_eq!(ROTATE_LEFT_RECT.1, ROTATE_ZOOM_RECT.1);
    }

    /// Rotating is a symmetric step around the podium pose, and the podium
    /// pose itself (facing the camera) is what a fresh screen shows.
    #[test]
    fn rotating_right_then_left_returns_to_the_podium_pose() {
        let podium = preview_rotation(0.0);
        assert!(podium.angle_between(Quat::from_rotation_y(PI)) < 1e-5);

        let view = CharCreatePreviewView::default();
        assert_eq!(view.yaw, 0.0);
        assert!(!view.zoomed_in);

        let turned = preview_rotation(PREVIEW_ROTATE_STEP);
        assert!(
            turned.angle_between(podium) > 1e-3,
            "a click turns the body"
        );
        let back = preview_rotation(PREVIEW_ROTATE_STEP - PREVIEW_ROTATE_STEP);
        assert!(back.angle_between(podium) < 1e-5);
        // 24 clicks are a full turn (the stated openroad step choice)
        assert!(((PI / PREVIEW_ROTATE_STEP) - 12.0).abs() < 1e-5);
    }

    /// The CH garment captions are `Protector` and `Armor` in the data, not the
    /// "Light Armor"/"Heavy Armor" we invented. The EU keys genuinely DO read
    /// "Light Armor"/"Heavy Armor", so only the CH pair was wrong (#304).
    #[test]
    fn chinese_garment_fallbacks_match_the_textdata() {
        let (ch_light_key, ch_light) = Garment::Light.label(Race::Chinese);
        assert_eq!(ch_light_key, "UIO_NEWCHAR_STT_LIGHT_ARMOR");
        assert_eq!(ch_light, "Protector");

        let (ch_heavy_key, ch_heavy) = Garment::Heavy.label(Race::Chinese);
        assert_eq!(ch_heavy_key, "UIO_NEWCHAR_STT_HEAVY_ARMOR");
        assert_eq!(ch_heavy, "Armor");

        // unchanged on purpose — these two are correct against the data
        assert_eq!(Garment::Light.label(Race::European).1, "Light Armor");
        assert_eq!(Garment::Heavy.label(Race::European).1, "Heavy Armor");
    }
}
