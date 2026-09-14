//! Free-PvP team panel — `res_ui/frpvp.2dt`, root `CNIFFreeBattlePvp` id 187.
//! Doc: `docs/re/ui/frpvp-window.md`.
//!
//! Idea: this is the tree's **first real `JMXV2DT` consumer**. Every other
//! 4th-generation window we ship transcribes its descriptor into Rust constants;
//! this one loads `res_ui/frpvp.2dt` through the asset server and builds the
//! panel out of the entries it finds, so the layout stays the data's. That is
//! the reusable half of this change — the arena, open-market and reputation
//! windows then cost only their own semantics.
//!
//! Three things the descriptor does not say in its record order, and which this
//! module therefore takes from the data rather than from the file layout:
//!
//! * **Rows are ordered by `y`, never by record index or `Id`.** The five team
//!   icons are records 11, 13, 12, 14, 15 but run red(111) / gray(141) /
//!   blue(172) / white(202) / yellow(232) down the panel — sorting by index puts
//!   blue above gray (doc §3a, the `targetmenu.2dt` trap).
//! * **A row's team comes from its art**, `frpvp_{red,gray,blue,white,yellow}`,
//!   not from its position in the file.
//! * **The wire byte is `{None=0, Red=1, Gray=2, Blue=3, White=4, Yellow=5}`** —
//!   four independent sources agree (`docs/re/systems/pvp-murder-state.md:53`).
//!   `packets`' `free_pvp: u8` has been parsed and unconsumed until now; this
//!   panel is its first consumer.
//!
//! Yellow is not a fifth team: its own string says the character becomes hostile
//! against everyone *including* the yellow group. We keep it as a distinct
//! free-for-all state rather than dressing it as a team (doc §3b).
//!
//! Stated deviations (ADR 0009):
//!
//! * **Anchored top-right instead of the authored `817,76`.** The authored
//!   origin is a point on the original's 1024x768 canvas; the original itself
//!   re-anchors windows at runtime, and pinning a docked panel to an absolute
//!   coordinate would put it off-screen at other resolutions. Internal geometry
//!   stays exactly as authored, panel-local.
//! * **Button state art (`_focus/_press/_disable`) is resolved by suffix**, not
//!   by four hardcoded paths — the descriptor names only the base art, and the
//!   original resolves the states by the same convention (doc §3c).
//! * **No join/leave request is sent.** The outbound opcode is unidentified
//!   (doc §9); picking a team raises [`RequestFreePvpTeam`] and nothing else
//!   consumes it yet. Inventing a packet would be inventing the wire.

use bevy::prelude::*;
use bevy::ui::UiTargetCamera;
use bevy::ui_widgets::{Activate, Button};

use crate::assets::twodt::{Jmxv2dtType, JMXV2DT};
use crate::assets::FontAssets;
use crate::plugins::hud::scale::hud_scale;
use crate::plugins::net::character_info::CharacterInfo;
use crate::plugins::player::Player;
use crate::plugins::textdata::ClientUiStrings;
use crate::scenes::SceneState;

/// The descriptor, as the exe spells it (`FUN_00786f30`'s
/// `res_ui\<file>.2dt` literal).
const DESCRIPTOR: &str = "media://res_ui/frpvp.2dt";
/// Where the descriptor's `Background` paths are rooted.
const ART_ROOT: &str = "media://interface/";
const CAPTION_FONT: f32 = 9.0;

/// One of the four Free-PvP teams, plus the yellow free-for-all state.
///
/// The order is the data's, triple-confirmed: `textuisystem.txt:3072-3076`
/// declares the five keys in it, the exe sets them from ascending addresses
/// `005a50db..005a5147`, and the descriptor's icons run in it top to bottom.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum FreePvpTeam {
    /// Team Red Hawk.
    Red,
    /// Team Black Turtle.
    Gray,
    /// Team Blue Dragon.
    Blue,
    /// Team White Tiger.
    White,
    /// Not a team: hostile against everyone, including Team Giraffe.
    Yellow,
}

impl FreePvpTeam {
    pub const ALL: [FreePvpTeam; 5] = [
        FreePvpTeam::Red,
        FreePvpTeam::Gray,
        FreePvpTeam::Blue,
        FreePvpTeam::White,
        FreePvpTeam::Yellow,
    ];

    /// CHARACTER_DATA's `free_pvp` byte: `0` is "not in Free-PvP", `1..=5` are
    /// the five states in declaration order. Anything else is a value we have
    /// never seen and is read as "not in Free-PvP" rather than guessed at.
    pub fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Red),
            2 => Some(Self::Gray),
            3 => Some(Self::Blue),
            4 => Some(Self::White),
            5 => Some(Self::Yellow),
            _ => None,
        }
    }

    /// The inverse of [`Self::from_wire`]; `None` is `0`.
    pub fn to_wire(team: Option<Self>) -> u8 {
        match team {
            None => 0,
            Some(Self::Red) => 1,
            Some(Self::Gray) => 2,
            Some(Self::Blue) => 3,
            Some(Self::White) => 4,
            Some(Self::Yellow) => 5,
        }
    }

    /// The art base name the descriptor uses for this state's icon.
    pub fn art_stem(self) -> &'static str {
        match self {
            Self::Red => "frpvp_red",
            Self::Gray => "frpvp_gray",
            Self::Blue => "frpvp_blue",
            Self::White => "frpvp_white",
            Self::Yellow => "frpvp_yellow",
        }
    }

    /// The team named by an icon entry's `Background`, whatever its record
    /// index — `interface\frpvp\frpvp_gray.ddj` is gray even if it is record 13.
    pub fn from_art(background: &str) -> Option<Self> {
        let lowered = background.to_ascii_lowercase().replace('\\', "/");
        let stem = lowered.rsplit('/').next()?.trim_end_matches(".ddj");
        Self::ALL.into_iter().find(|team| team.art_stem() == stem)
    }

    /// The textuisystem key describing this state.
    pub fn string_key(self) -> &'static str {
        match self {
            Self::Red => "UIIT_STT_FRPVP_RED",
            Self::Gray => "UIIT_STT_FRPVP_GRAY",
            Self::Blue => "UIIT_STT_FRPVP_BLUE",
            Self::White => "UIIT_STT_FRPVP_WHITE",
            Self::Yellow => "UIIT_STT_FRPVP_YELLOW",
        }
    }
}

/// A team pick the player made. Nothing consumes it yet — the outbound opcode
/// is unidentified (doc §9), and this is the seam where it lands.
#[derive(Message, Clone, Copy, Debug)]
pub struct RequestFreePvpTeam(pub Option<FreePvpTeam>);

#[derive(Resource, Default)]
pub struct FreePvpState {
    /// The team the server says we are on, from CHARACTER_DATA's `free_pvp`.
    pub team: Option<FreePvpTeam>,
    /// Whether the slot subtree (the five rows and the cancel button) is shown.
    /// The collapse button toggles it.
    pub expanded: bool,
}

impl FreePvpState {
    /// The panel exists only while the character is in Free-PvP mode — the
    /// original has no other way to reach it (its window is registry-free and
    /// its whole reachability runs through inbound `0xB516`).
    pub fn visible(&self) -> bool {
        self.team.is_some()
    }
}

#[derive(Resource)]
struct FreePvpDescriptor(Handle<JMXV2DT>);

#[derive(Component)]
pub struct FreePvpPanel;

#[derive(Component)]
struct FreePvpBody;

#[derive(Component, Clone, Copy)]
struct FreePvpRow(FreePvpTeam);

#[derive(Component)]
struct FreePvpCollapse;

/// `frpvp\frpvp_red.ddj` -> `media://interface/frpvp/frpvp_red.ddj`.
///
/// Descriptor art paths are CP949 Windows paths relative to `Media/interface`;
/// our asset paths are lowercase and forward-slashed.
fn art_path(background: &str) -> String {
    format!(
        "{ART_ROOT}{}",
        background.to_ascii_lowercase().replace('\\', "/")
    )
}

/// The rows of a `frpvp.2dt`-shaped descriptor, top to bottom.
///
/// Sorted by `y` and keyed by art, because the record order is neither
/// (doc §3a). Returns the team, its icon rect and the icon's art path.
fn rows(descriptor: &JMXV2DT) -> Vec<(FreePvpTeam, Rect, String)> {
    let mut rows: Vec<(FreePvpTeam, Rect, String)> = descriptor
        .entries()
        .iter()
        .filter_map(|entry| {
            let team = FreePvpTeam::from_art(entry.background())?;
            Some((
                team,
                descriptor.local_rect(entry),
                art_path(entry.background()),
            ))
        })
        .collect();
    rows.sort_by(|a, b| a.1.min.y.total_cmp(&b.1.min.y));
    rows
}

/// The check boxes (`CNIFCheckBox`), top to bottom — one per row.
fn check_boxes(descriptor: &JMXV2DT) -> Vec<Rect> {
    let mut boxes: Vec<Rect> = descriptor
        .entries()
        .iter()
        .filter(|entry| entry.ni_type() == Some(Jmxv2dtType::CNIFCheckBox))
        .map(|entry| descriptor.local_rect(entry))
        .collect();
    boxes.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
    boxes
}

fn load_descriptor(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(FreePvpDescriptor(asset_server.load(DESCRIPTOR)));
}

/// The server's `free_pvp` byte is the panel's whole input.
fn sync_free_pvp_state(
    players: Query<&CharacterInfo, With<Player>>,
    mut state: ResMut<FreePvpState>,
) {
    let Ok(info) = players.single() else { return };
    let team = info
        .stats
        .as_ref()
        .and_then(|stats| FreePvpTeam::from_wire(stats.free_pvp));
    if state.team != team {
        state.team = team;
        // A panel that just appeared starts expanded; the collapse button is
        // the only thing that closes it.
        state.expanded = team.is_some();
    }
}

/// Build (or rebuild) the panel from the descriptor whenever the state or the
/// asset changes. Without the descriptor there is no panel — the layout is the
/// data's, and we do not carry a transcribed fallback of it.
#[allow(clippy::too_many_arguments)]
fn rebuild_free_pvp_panel(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    descriptors: Res<Assets<JMXV2DT>>,
    handle: Option<Res<FreePvpDescriptor>>,
    state: Res<FreePvpState>,
    panels: Query<Entity, With<FreePvpPanel>>,
    cameras: Query<Entity, With<Camera2d>>,
) {
    let changed = state.is_changed() || descriptors.is_changed();
    if !changed {
        return;
    }
    for panel in panels.iter() {
        commands.entity(panel).despawn();
    }
    if !state.visible() {
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
    let root_size = root.rect().size();

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

    // The panel root is the authored size, but docked top-right (see the module
    // note); everything inside keeps the authored panel-local geometry.
    let panel = commands
        .spawn((
            FreePvpPanel,
            Name::from("Free PvP Panel"),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(8.0 * s),
                top: Val::Px(8.0 * s),
                width: Val::Px(root_size.x * s),
                height: Val::Px(root_size.y * s),
                ..default()
            },
            GlobalZIndex(30),
            UiTargetCamera(camera),
        ))
        .id();

    // Header: the frame art and the collapse button, both direct children of
    // the root in the descriptor.
    for entry in descriptor.children_of(root.id()) {
        if entry.background().is_empty() {
            continue;
        }
        let rect = descriptor.local_rect(entry);
        let art = image(&art_path(entry.background()));
        if entry.ni_type() == Some(Jmxv2dtType::CNIFButton) {
            commands
                .entity(panel)
                .with_child((FreePvpCollapse, Button, node(rect), art))
                .observe(|_: On<Activate>, mut state: ResMut<FreePvpState>| {
                    state.expanded = !state.expanded;
                });
        } else {
            commands
                .entity(panel)
                .with_child((node(rect), art, Pickable::IGNORE));
        }
    }

    if !state.expanded {
        return;
    }

    // Body: the slot subtree — the bottom frame, the five rows and the cancel
    // button. Its entries are absolute like every other rect, so they need no
    // second origin.
    let body = commands
        .spawn((
            FreePvpBody,
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Pickable::IGNORE,
        ))
        .id();
    commands.entity(panel).add_child(body);

    let Some(slot) = descriptor.entry_by_name("CNIFFreeBattlePvpSlot") else {
        return;
    };
    for entry in descriptor.children_of(slot.id()) {
        let rect = descriptor.local_rect(entry);
        match entry.ni_type() {
            Some(Jmxv2dtType::CNIFStatic) if !entry.background().is_empty() => {
                commands.entity(body).with_child((
                    node(rect),
                    image(&art_path(entry.background())),
                    Pickable::IGNORE,
                ));
            }
            Some(Jmxv2dtType::CNIFButton) => {
                let caption = ui_strings.get_or(entry.text(), "Cancel").to_string();
                commands
                    .entity(body)
                    .with_child((
                        Button,
                        node(rect),
                        image(&art_path(entry.background())),
                        children![(
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
                                TextColor(entry.color()),
                            )],
                        )],
                    ))
                    .observe(
                        |_: On<Activate>, mut requests: MessageWriter<RequestFreePvpTeam>| {
                            requests.write(RequestFreePvpTeam(None));
                        },
                    );
            }
            _ => {}
        }
    }

    // The rows: a check box per row (sorted by y, like the icons) and the team
    // icon it belongs to.
    let boxes = check_boxes(descriptor);
    for (index, (team, icon_rect, icon_art)) in rows(descriptor).into_iter().enumerate() {
        if let Some(box_rect) = boxes.get(index) {
            // The descriptor names only `com_radiobutton_on.ddj`; there is no
            // authored "off" art, so an unpicked row draws no glyph rather than
            // a guessed one. The hit rect is there either way.
            let mut row = commands.spawn((FreePvpRow(team), Button, node(*box_rect)));
            if state.team == Some(team) {
                row.insert(image("media://interface/ifcommon/com_radiobutton_on.ddj"));
            }
            row.observe(
                |activate: On<Activate>,
                 rows: Query<&FreePvpRow>,
                 mut requests: MessageWriter<RequestFreePvpTeam>| {
                    if let Ok(row) = rows.get(activate.entity) {
                        requests.write(RequestFreePvpTeam(Some(row.0)));
                    }
                },
            );
            let row = row.id();
            commands.entity(body).add_child(row);
        }
        commands
            .entity(body)
            .with_child((node(icon_rect), image(&icon_art), Pickable::IGNORE));
    }
}

fn cleanup_free_pvp_panel(
    mut commands: Commands,
    panels: Query<Entity, With<FreePvpPanel>>,
    mut state: ResMut<FreePvpState>,
) {
    for panel in panels.iter() {
        commands.entity(panel).despawn();
    }
    *state = FreePvpState::default();
}

pub struct FreePvpPlugin;

impl Plugin for FreePvpPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FreePvpState>()
            .add_message::<RequestFreePvpTeam>()
            .add_systems(OnEnter(SceneState::GameWorld), load_descriptor)
            .add_systems(OnExit(SceneState::GameWorld), cleanup_free_pvp_panel)
            .add_systems(
                Update,
                (sync_free_pvp_state, rebuild_free_pvp_panel)
                    .chain()
                    .run_if(in_state(SceneState::GameWorld)),
            );
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// `{None=0, Red=1, Gray=2, Blue=3, White=4, Yellow=5}` — the mapping four
    /// independent sources agree on. Unknown bytes read as "not in Free-PvP"
    /// instead of being guessed at.
    #[test]
    fn the_wire_byte_maps_to_the_declared_order() {
        assert_eq!(FreePvpTeam::from_wire(0), None);
        for (byte, team) in (1u8..).zip(FreePvpTeam::ALL) {
            assert_eq!(FreePvpTeam::from_wire(byte), Some(team));
            assert_eq!(FreePvpTeam::to_wire(Some(team)), byte);
        }
        assert_eq!(FreePvpTeam::from_wire(6), None);
        assert_eq!(FreePvpTeam::from_wire(0xFF), None);
        assert_eq!(FreePvpTeam::to_wire(None), 0);
    }

    /// A row's team is read from its art, which is what makes the record order
    /// (11, 13, 12, 14, 15) harmless.
    #[test]
    fn a_row_takes_its_team_from_its_art() {
        assert_eq!(
            FreePvpTeam::from_art("frpvp\\frpvp_gray.ddj"),
            Some(FreePvpTeam::Gray)
        );
        // the descriptor's own casing and separators vary; both are accepted
        assert_eq!(
            FreePvpTeam::from_art("FRPVP\\FRPVP_BLUE.DDJ"),
            Some(FreePvpTeam::Blue)
        );
        assert_eq!(
            FreePvpTeam::from_art("interface/frpvp/frpvp_yellow.ddj"),
            Some(FreePvpTeam::Yellow)
        );
        // the panel's other arts are not rows
        assert_eq!(FreePvpTeam::from_art("frpvp\\frpvp_frame01.ddj"), None);
        assert_eq!(FreePvpTeam::from_art("frpvp\\frpvp_button.ddj"), None);
        assert_eq!(FreePvpTeam::from_art(""), None);
    }

    #[test]
    fn art_paths_are_media_relative_and_forward_slashed() {
        assert_eq!(
            art_path("frpvp\\frpvp_frame02.ddj"),
            "media://interface/frpvp/frpvp_frame02.ddj"
        );
        assert_eq!(
            art_path("ifcommon\\com_m_button02.ddj"),
            "media://interface/ifcommon/com_m_button02.ddj"
        );
    }

    /// Every state has its own icon and its own string; nothing shares.
    #[test]
    fn each_state_has_its_own_art_and_string() {
        let mut stems: Vec<&str> = FreePvpTeam::ALL.iter().map(|t| t.art_stem()).collect();
        stems.sort_unstable();
        stems.dedup();
        assert_eq!(stems.len(), 5);
        let mut keys: Vec<&str> = FreePvpTeam::ALL.iter().map(|t| t.string_key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 5);
        // yellow is a state, not a team, and the data says so in its own string
        assert_eq!(FreePvpTeam::Yellow.string_key(), "UIIT_STT_FRPVP_YELLOW");
    }

    /// The panel exists only while the character is in Free-PvP mode.
    #[test]
    fn the_panel_follows_the_wire_byte() {
        let mut state = FreePvpState::default();
        assert!(!state.visible());
        state.team = FreePvpTeam::from_wire(2);
        assert!(state.visible());
        assert_eq!(state.team, Some(FreePvpTeam::Gray));
        state.team = FreePvpTeam::from_wire(0);
        assert!(!state.visible());
    }
}
