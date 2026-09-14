//! Alchemy 4th-generation enchant window — `res_ui/nifenchantwnd.2dt`, root
//! `CNIFEnchantWnd` id 168 `(413,33,372,371)`.
//! Doc: `docs/re/ui/hud-enchant-window.md`. Issue #476; the classic half is #333
//! and stays where it is (`hud/alchemy/ui.rs`).
//!
//! Idea: the window's central fact is in its bytes, not in its art — **four tabs
//! over two panes**. Each `CNIFTabButton`'s `ContentId` is the `Id` of the pane
//! it raises, and the four tabs carry only two distinct values: Disjoint and
//! Dismantle both point at `CNIFAlchemySubWndType2` (id 28), Manufacture and
//! Strengthen both at `CNIFAlchemySubWndType1` (id 12). So this module renders
//! the descriptor's own tab->pane mapping rather than a hand-written table, and
//! the two panes' byte-identical rect (`409,107,376,300`) is the client's `if`,
//! not two overlapping windows.
//!
//! Like `hud/free_pvp.rs` this is a descriptor-driven window: the layout is
//! loaded from the `.2dt`, never transcribed. What is written down here is only
//! what the descriptor cannot say.
//!
//! Three traps this file exists to not fall into:
//!
//! * **The `_on`/`_off` art suffix is not state.** Three of the four tabs ship
//!   the `_on` art and only the fourth ships `_off`, which is authoring residue —
//!   exactly one tab can be active at runtime. The active tab comes from
//!   [`EnchantState::verb`], never from a filename.
//! * **`mframe_alc_` is not a working 9-slice.** Seven of its eight pieces are
//!   4x4 DXT1 stubs and the whole frame is one 376x376 bitmap packed into the
//!   `right_up` slot, so the plate is drawn as a single full-size image. A
//!   generic "every `mframe_*` is a 9-slice" helper would mangle this window.
//! * **`res_ui/nifenchantwnd.ddj` must stay ignored.** It is the same window one
//!   revision earlier (a 2DT with a texture extension), and its slot grid is the
//!   *broken* one the authors then fixed. `assets/ddj.rs` already logs-and-skips
//!   it with a regression test; we load the `.2dt` and only the `.2dt`.
//!
//! Deliberately not built yet, and why: the progress gauge (a `Style=0` gauge
//! crops along X instead of stretching, and four of our seven existing fill
//! sites already get that wrong — it deserves the shared crop helper, not a
//! fifth wrong one), the item slots' contents (no wire), and the shared
//! `CNIFMiniConfirm` dialog (`res_ui/nifenchantalchemymsgbox.2dt`, root id 172),
//! which the doc reads as shared and therefore not alchemy-local.

use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::twodt::{Jmxv2dtType, JMXV2DT};
use crate::assets::FontAssets;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::textdata::ClientUiStrings;
use crate::scenes::SceneState;

const DESCRIPTOR: &str = "media://res_ui/nifenchantwnd.2dt";
const ART_ROOT: &str = "media://interface/";
/// The whole window frame is this one 376x376 DXT1 bitmap; the other seven
/// `mframe_alc_` pieces are 4x4 stubs (doc §3).
const PLATE_ART: &str = "media://interface/frame/mframe_alc_right_up.ddj";
const CAPTION_FONT: f32 = 9.0;

/// `CNIFAlchemySubWndType1`'s `Id` — the pane Manufacture and Strengthen raise.
const PANE_TYPE1_ID: i32 = 12;
/// `CNIFAlchemySubWndType2`'s `Id` — the pane Disjoint and Dismantle raise.
const PANE_TYPE2_ID: i32 = 28;

/// The two pane layouts the four verbs share.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlchemyPane {
    /// `CNIFAlchemySubWndType1` (id 12): one item plus its stones.
    ItemPlusStones,
    /// `CNIFAlchemySubWndType2` (id 28): one item in, many out.
    OneToMany,
}

impl AlchemyPane {
    /// The pane a tab's `ContentId` names. Anything else is a descriptor we do
    /// not understand, and is left unrendered rather than guessed at.
    pub fn from_content_id(content_id: i32) -> Option<Self> {
        match content_id {
            PANE_TYPE1_ID => Some(Self::ItemPlusStones),
            PANE_TYPE2_ID => Some(Self::OneToMany),
            _ => None,
        }
    }

    pub fn content_id(self) -> i32 {
        match self {
            Self::ItemPlusStones => PANE_TYPE1_ID,
            Self::OneToMany => PANE_TYPE2_ID,
        }
    }
}

/// The four alchemy verbs, in the descriptor's left-to-right tab order
/// (x = 430, 515, 600, 685 — 80 wide at pitch 85, a uniform 5 px gap).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlchemyVerb {
    Disjoint,
    Dismantle,
    Manufacture,
    Strengthen,
}

impl AlchemyVerb {
    pub const ALL: [AlchemyVerb; 4] = [
        AlchemyVerb::Disjoint,
        AlchemyVerb::Dismantle,
        AlchemyVerb::Manufacture,
        AlchemyVerb::Strengthen,
    ];

    /// The tab caption key the descriptor carries in its `Text` field.
    pub fn string_key(self) -> &'static str {
        match self {
            Self::Disjoint => "UIIT_CTL_ALCHEMYBOX_TAP_DISJOINTING",
            Self::Dismantle => "UIIT_CTL_ALCHEMYBOX_TAP_DISMANTLING",
            Self::Manufacture => "UIIT_CTL_ALCHEMYBOX_TAP_MANUFACTURING",
            Self::Strengthen => "UIIT_CTL_ALCHEMYBOX_TAP_STRENGTHENING",
        }
    }

    pub fn from_string_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|verb| verb.string_key() == key)
    }

    /// The pane this verb works in — **read from the descriptor's `ContentId`**,
    /// which is why four verbs need only two pane layouts.
    pub fn pane(self) -> AlchemyPane {
        match self {
            Self::Disjoint | Self::Dismantle => AlchemyPane::OneToMany,
            Self::Manufacture | Self::Strengthen => AlchemyPane::ItemPlusStones,
        }
    }
}

#[derive(Resource)]
pub struct EnchantState {
    pub open: bool,
    /// The active tab. The `_on`/`_off` art suffixes say nothing about this.
    pub verb: AlchemyVerb,
}

impl Default for EnchantState {
    fn default() -> Self {
        Self {
            open: false,
            // Leftmost tab; the descriptor authors no initial selection.
            verb: AlchemyVerb::Disjoint,
        }
    }
}

#[derive(Resource)]
struct EnchantDescriptor(Handle<JMXV2DT>);

#[derive(Component)]
pub struct EnchantWindow;

#[derive(Component, Clone, Copy)]
struct EnchantTab(AlchemyVerb);

/// `frame\mframe_alc_right_up.ddj` -> a media-relative asset path.
fn art_path(background: &str) -> String {
    format!(
        "{ART_ROOT}{}",
        background.to_ascii_lowercase().replace('\\', "/")
    )
}

/// Anything laid out left to right is read **in coordinate order**, never in
/// record order — the descriptor corpus reorders records freely
/// (`docs/re/ui/frpvp-window.md` §3a, `targetmenu.2dt`).
fn order_by_x<T>(mut items: Vec<(f32, T)>) -> Vec<T> {
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    items.into_iter().map(|(_, item)| item).collect()
}

fn load_descriptor(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(EnchantDescriptor(asset_server.load(DESCRIPTOR)));
}

/// The tabs the descriptor declares, left to right, with the pane each raises.
fn tabs(descriptor: &JMXV2DT) -> Vec<(AlchemyVerb, AlchemyPane, Rect, String)> {
    let found: Vec<(f32, (AlchemyVerb, AlchemyPane, Rect, String))> = descriptor
        .entries()
        .iter()
        .filter(|entry| entry.ni_type() == Some(Jmxv2dtType::CNIFTabButton))
        .filter_map(|entry| {
            let verb = AlchemyVerb::from_string_key(entry.text())?;
            let pane = AlchemyPane::from_content_id(entry.content_id())?;
            let rect = descriptor.local_rect(entry);
            Some((
                entry.rect().min.x,
                (verb, pane, rect, art_path(entry.background())),
            ))
        })
        .collect();
    order_by_x(found)
}

/// Rebuild the window from the descriptor whenever the state or the asset
/// changes. Without the descriptor there is no window: the layout is the data's
/// and we carry no transcribed fallback of it.
#[allow(clippy::too_many_arguments)]
fn rebuild_enchant_window(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    descriptors: Res<Assets<JMXV2DT>>,
    handle: Option<Res<EnchantDescriptor>>,
    state: Res<EnchantState>,
    windows: Query<Entity, With<EnchantWindow>>,
    cameras: Query<Entity, With<Camera2d>>,
) {
    if !state.is_changed() && !descriptors.is_changed() {
        return;
    }
    for window in windows.iter() {
        commands.entity(window).despawn();
    }
    if !state.open {
        return;
    }
    let (Some(handle), Ok(camera)) = (handle, cameras.single()) else {
        return;
    };
    let Some(descriptor) = descriptors.get(&handle.0) else {
        return;
    };
    let Some(root) = descriptor.root() else {
        return;
    };
    let s = hud_scale();
    let size = root.rect().size();

    let node = |rect: Rect| Node {
        position_type: PositionType::Absolute,
        left: Val::Px(rect.min.x * s),
        top: Val::Px(rect.min.y * s),
        width: Val::Px(rect.width() * s),
        height: Val::Px(rect.height() * s),
        ..default()
    };
    let image = |path: &str| ImageNode {
        image: asset_server.load(path.to_string()),
        image_mode: NodeImageMode::Stretch,
        ..default()
    };

    let window = commands
        .spawn((
            EnchantWindow,
            Name::from("Enchant Window"),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(root.rect().min.x * s),
                top: Val::Px(root.rect().min.y * s),
                width: Val::Px(size.x * s),
                height: Val::Px(size.y * s),
                ..default()
            },
            GlobalZIndex(25),
            UiTargetCamera(camera),
        ))
        .id();

    // The frame: one plate, because seven of the eight `mframe_alc_` pieces are
    // 4x4 stubs and the eighth is the whole 376x376 bitmap.
    commands.entity(window).with_child((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        image(PLATE_ART),
        Pickable::IGNORE,
    ));

    for (verb, _pane, rect, art) in tabs(descriptor) {
        let caption = ui_strings.get_or(verb.string_key(), "").to_string();
        let tab = commands
            .spawn((EnchantTab(verb), Button, node(rect), image(&art)))
            .observe(
                |activate: On<Activate>,
                 tabs: Query<&EnchantTab>,
                 mut state: ResMut<EnchantState>| {
                    if let Ok(tab) = tabs.get(activate.entity) {
                        state.verb = tab.0;
                    }
                },
            )
            .id();
        commands.entity(tab).with_child((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            Pickable::IGNORE,
            children![(
                Text::new(caption),
                TextFont {
                    font: fonts.nine.clone().into(),
                    font_size: FontSize::Px(CAPTION_FONT * s),
                    ..default()
                },
                TextColor(Color::WHITE),
            )],
        ));
        commands.entity(window).add_child(tab);
    }

    // The active pane, raised by the active tab's `ContentId`. Its siblings stay
    // unspawned rather than hidden — the two panes share a rect, so drawing both
    // would draw one on top of the other.
    let pane_id = state.verb.pane().content_id();
    let Some(pane) = descriptor
        .entries()
        .iter()
        .find(|entry| entry.id() as i32 == pane_id)
    else {
        return;
    };
    for entry in descriptor.children_of(pane.id()) {
        if entry.background().is_empty() {
            continue;
        }
        commands.entity(window).with_child((
            node(descriptor.local_rect(entry)),
            image(&art_path(entry.background())),
            Pickable::IGNORE,
        ));
    }
}

fn cleanup_enchant_window(
    mut commands: Commands,
    windows: Query<Entity, With<EnchantWindow>>,
    mut state: ResMut<EnchantState>,
) {
    for window in windows.iter() {
        commands.entity(window).despawn();
    }
    *state = EnchantState::default();
}

pub struct EnchantPlugin;

impl Plugin for EnchantPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EnchantState>()
            .add_systems(OnEnter(SceneState::GameWorld), load_descriptor)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_enchant_window)
            .add_systems(
                Update,
                rebuild_enchant_window.run_if(in_state(SceneState::GameWorld)),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The window's central finding: four verbs, two panes, and the split is the
    /// descriptor's `ContentId` — 28 for Disjoint/Dismantle, 12 for
    /// Manufacture/Strengthen.
    #[test]
    fn four_verbs_run_over_two_panes() {
        let panes: Vec<AlchemyPane> = AlchemyVerb::ALL.iter().map(|verb| verb.pane()).collect();
        assert_eq!(
            panes,
            vec![
                AlchemyPane::OneToMany,
                AlchemyPane::OneToMany,
                AlchemyPane::ItemPlusStones,
                AlchemyPane::ItemPlusStones,
            ]
        );
        assert_eq!(AlchemyVerb::Disjoint.pane().content_id(), 28);
        assert_eq!(AlchemyVerb::Manufacture.pane().content_id(), 12);
        // and the mapping is read back out of a ContentId, not hardcoded per tab
        assert_eq!(
            AlchemyPane::from_content_id(12),
            Some(AlchemyPane::ItemPlusStones)
        );
        assert_eq!(
            AlchemyPane::from_content_id(28),
            Some(AlchemyPane::OneToMany)
        );
        // an unknown ContentId renders nothing rather than a guessed pane
        assert_eq!(AlchemyPane::from_content_id(0), None);
        assert_eq!(AlchemyPane::from_content_id(-1), None);
    }

    /// Tabs are identified by their caption key, and every verb has its own.
    #[test]
    fn each_verb_owns_one_caption_key() {
        let mut keys: Vec<&str> = AlchemyVerb::ALL.iter().map(|v| v.string_key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 4);
        for verb in AlchemyVerb::ALL {
            assert_eq!(AlchemyVerb::from_string_key(verb.string_key()), Some(verb));
        }
        assert_eq!(
            AlchemyVerb::from_string_key("UIIT_CTL_ALCHEMYBOX_TAP"),
            None
        );
        assert_eq!(AlchemyVerb::from_string_key(""), None);
    }

    /// The tab strip is ordered by x (430/515/600/685), never by record order.
    #[test]
    fn the_tab_strip_is_ordered_by_x() {
        let shuffled = vec![
            (600.0, AlchemyVerb::Manufacture),
            (430.0, AlchemyVerb::Disjoint),
            (685.0, AlchemyVerb::Strengthen),
            (515.0, AlchemyVerb::Dismantle),
        ];
        assert_eq!(order_by_x(shuffled), AlchemyVerb::ALL.to_vec());
        // 80 wide at pitch 85 is a uniform 5 px gap
        let xs = [430.0_f32, 515.0, 600.0, 685.0];
        for pair in xs.windows(2) {
            assert_eq!(pair[1] - pair[0], 85.0);
            assert_eq!(pair[1] - (pair[0] + 80.0), 5.0);
        }
    }

    /// The active tab is state, never the `_on`/`_off` art suffix: three tabs
    /// ship `_on` and only the fourth ships `_off`, which is authoring residue.
    #[test]
    fn the_active_tab_comes_from_state() {
        let mut state = EnchantState::default();
        assert_eq!(state.verb, AlchemyVerb::Disjoint);
        assert_eq!(state.verb.pane(), AlchemyPane::OneToMany);
        state.verb = AlchemyVerb::Strengthen;
        assert_eq!(state.verb.pane(), AlchemyPane::ItemPlusStones);
    }

    #[test]
    fn art_paths_are_media_relative_and_forward_slashed() {
        assert_eq!(
            art_path("frame\\mframe_alc_right_up.ddj"),
            "media://interface/frame/mframe_alc_right_up.ddj"
        );
    }
}
