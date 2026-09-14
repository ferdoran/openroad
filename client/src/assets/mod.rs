use std::io::Cursor;

use bevy::asset::AssetApp;
use bevy::prelude::{
    info, warn, App, AssetServer, Font, Handle, Local, Plugin, Res, ResMut, Resource, Update,
};
use bevy_asset_loader::prelude::AssetCollection;
use unidecode::unidecode;

use crate::assets::ainav::{AINavData, AinavLoader};
use crate::assets::ban::{BanLoader, JMXVBAN};
use crate::assets::bms::mesh::JMXVBMS;
use crate::assets::bms::BmsLoader;
use crate::assets::bmt::material::{BmtLoader, SroMaterial, JMXVBMT};
use crate::assets::bsk::{BskLoader, JMXVBSK};
use crate::assets::bsr::loader::BsrLoaderV2;
use crate::assets::bsr::resource::SroResource;
use crate::assets::char_select_scene::CharSelectScene;
use crate::assets::cpd::{CpdLoader, JMXVCPD};
use crate::assets::ddj::JMXVDDJ;
use crate::assets::dof::{DofLoader, JMXVDOF};
use crate::assets::efp::loader::EfpLoader;
use crate::assets::efp::JMXVEFF;
use crate::assets::ifo::IFOAsset;
use crate::assets::intro_scene::IntroScene;
use crate::assets::m::block_splat_material::TerrainBlockSplatMaterial;
use crate::assets::m::loader::MLoader;
use crate::assets::m::JMXVMAPM;
use crate::assets::mfo::JMXVMFO;
use crate::assets::nvm::loader::NvmLoader;
use crate::assets::nvm::JMXVNVM;
use crate::assets::o::JMXVMAPO;
use crate::assets::o2::JMXVMAPO2;
use crate::assets::resinfo::interface_text::loader::InterfaceTextLoader;
use crate::assets::resinfo::interface_text::InterfaceText;
use crate::assets::t::{TLoader, JMXVMAPT};
use crate::assets::textdata::characterdata::CharacterData;
use crate::assets::textdata::{Textdata, TextdataLoader};
use crate::assets::twodt::JMXV2DT;
use crate::plugins::config::ClientConfig;

// Credits to DaxterSoul
// https://github.com/DummkopfOfHachtenduden/SilkroadDoc/wiki/Formats
pub mod ainav;
pub mod ban;
pub mod bms;
pub mod bmt;
pub mod bsk;
pub mod bsr;
pub mod char_select_scene;
pub mod cpd;
pub mod ddj;
pub mod dof;
pub mod efp;
pub mod ifo;
pub mod intro_scene;
pub mod m;
pub mod mfo;
pub mod nvm;
pub mod o;
pub mod o2;
pub mod resinfo;
pub mod t;
pub mod tile_tint;
pub mod twodt;

pub mod textdata;

pub type Str64 = String; //[char; 64];
pub type Str128 = String; //[char; 128];
pub type Str256 = String; //[char; 256];
/// D3DCOLOR: **ARGB**, not RGBA — the high byte is alpha.
pub type Argb8888 = u32;

pub fn read_str_and_jump(cursor: &mut Cursor<&[u8]>, size: u64) -> String {
    //println!("Str Size {:?}", size);
    if size < 1 {
        return String::from("");
    }

    let start = cursor.position() as usize;
    let next_position = cursor.position() + size;
    let end = next_position as usize;

    let bytes = cursor.get_ref();
    let splice = &(*bytes)[start..end];
    let str = read_str(splice);

    cursor.set_position(next_position);
    str
}
pub fn read_str(bytes: &[u8]) -> String {
    let mut chars: Vec<char> = Vec::with_capacity(bytes.len());

    //println!("Bytes Length {:?}", bytes.len());
    for b in bytes {
        let c = *b as char;
        if c == '\0' || c == 'ý' {
            continue;
        }
        chars.push(c);
        //print!("{:?} ", c);
    }

    let str = chars.iter().cloned().collect::<String>().to_owned();
    unidecode(str.as_str()) // todo maybe not needed
                            //format!("{}->{}",str.as_str(),unidecode(str.as_str()))
}

pub struct SroAssetStructsPlugin;
impl Plugin for SroAssetStructsPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<JMXV2DT>()
            .init_asset_loader::<twodt::TwoDtLoader>()
            .init_asset::<JMXVDDJ>()
            .init_asset_loader::<ddj::DDJLoader>()
            .init_asset::<JMXVMFO>()
            .init_asset_loader::<mfo::MFOLoader>()
            .init_asset::<IFOAsset>()
            // .init_asset::<JMXVENVI>()
            // .init_asset::<ObjectInfoIndex>()
            // .init_asset::<TileInfoIndex>()
            .init_asset_loader::<ifo::IFOLoader>()
            .init_asset::<JMXVMAPM>()
            .init_asset_loader::<MLoader>()
            .init_asset::<JMXVMAPO2>()
            .init_asset_loader::<o2::O2Loader>()
            .init_asset::<JMXVMAPO>()
            .init_asset_loader::<o::OLoader>()
            .init_asset::<JMXVMAPT>()
            .init_asset_loader::<TLoader>()
            // .init_asset::<JMXVRES>()
            // .init_asset_loader::<bsr::BsrLoader>()
            .init_asset::<SroResource>()
            .init_asset_loader::<BsrLoaderV2>()
            .init_asset::<JMXVBMS>()
            .init_asset_loader::<BmsLoader>()
            .init_asset::<JMXVCPD>()
            .init_asset_loader::<CpdLoader>()
            // resinfo (#477). Registered *before* `Textdata` on purpose: both
            // loaders claim `txt`, and `AssetLoaders::find` only falls back to
            // the extension when the requested asset type is ambiguous, taking
            // the **last** registered loader for that extension
            // (`bevy_asset-0.19.1/src/server/loaders.rs:154-236`). Each of these
            // two types has exactly one loader, so typed loads resolve by type
            // and never reach that fallback; keeping `Textdata` last leaves the
            // untyped `.txt` fallback pointing where it pointed before.
            .init_asset::<InterfaceText>()
            .init_asset_loader::<InterfaceTextLoader>()
            .init_asset::<JMXVBMT>()
            .init_asset::<SroMaterial>()
            .init_asset_loader::<BmtLoader>()
            .init_asset::<TerrainBlockSplatMaterial>()
            .init_asset::<JMXVBAN>()
            .init_asset_loader::<BanLoader>()
            .init_asset::<JMXVBSK>()
            .init_asset_loader::<BskLoader>()
            .init_asset::<JMXVNVM>()
            .init_asset_loader::<NvmLoader>()
            .init_asset::<JMXVDOF>()
            .init_asset_loader::<DofLoader>()
            .init_asset::<AINavData>()
            .init_asset_loader::<AinavLoader>()
            .init_asset::<JMXVEFF>()
            .init_asset_loader::<EfpLoader>()
            .init_asset::<IntroScene>()
            .init_asset_loader::<IntroScene>()
            .init_asset::<CharSelectScene>()
            .init_asset_loader::<CharSelectScene>()
            .init_asset::<CharacterData>()
            .init_asset::<Textdata>()
            .init_asset_loader::<TextdataLoader>()
            .init_asset::<crate::assets::resinfo::item_rare::ItemRareTable>()
            .init_asset_loader::<crate::assets::resinfo::item_rare::ItemRareLoader>()
            .add_systems(Update, apply_pk2_face);
    }
}

// `MusicAssets` (two hard-coded tracks, never registered with a loading state
// and never played) lived here until #771: zone music is chosen by the
// `effectenvsnd` table at run time, so a compile-time collection of two of the
// 45 tracks cannot express it. Removed rather than revived.

/// The original's UI faces, which ship in the user's own `Media.pk2` under
/// `Media/fonts/` — three TrueType files beside the three `0/i/y.dat` bitmap
/// stubs (100 / 88 / 124 bytes, three glyphs in total, so there is no `.dat`
/// font to load; `docs/re/ui/localization-and-fonts.md` §3).
///
/// Roles are read off the file names themselves, which is all the data
/// supports: **which `FontIndex` selects which face is `[U]`** (§9-U1), so
/// every slot below resolves through [`FontRole::path`] — a later resolution
/// is a one-file change, and no size ladder is invented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontRole {
    /// 기본서체 — the default UI face (family 굵은으뜸체).
    DefaultUi,
    /// 영문서체 — the latin face (family Arial Rounded MT Bold).
    Latin,
    /// 채팅서체 — the chat face (family 가는으뜸체).
    Chat,
}

impl FontRole {
    /// PK2 asset path for this role. Names are the archive's own, decoded from
    /// EUC-KR by `bevy_pk2` (`pk2/entry.rs:105`), so the precomposed Hangul
    /// literals here are what the archive index yields.
    pub fn path(self) -> &'static str {
        match self {
            FontRole::DefaultUi => "media://fonts/기본서체.ttf",
            FontRole::Latin => "media://fonts/영문서체.ttf",
            FontRole::Chat => "media://fonts/채팅서체.ttf",
        }
    }
}

/// The bundled UI face: **Arimo**, OFL 1.1, vendored at
/// `assets/fonts/Arimo-Variable.ttf` with its license beside it.
///
/// Three properties earned it the slot:
///
/// * **It is an Arial.** Arimo is metric-compatible with Arial (it is the
///   Chrome OS substitute, and Liberation Sans 2.x derives from it), so the UI
///   reads as the Arial the HUD was asked for without shipping a proprietary
///   Monotype file. This repository ships OFL faces only.
/// * **It is variable** (`wght` axis). Bevy's [`FontWeight`] only synthesizes
///   on variable fonts, so one file now covers both the body text and the bold
///   emphasis that used to need a second face.
/// * **Its default is Regular (400)**, where the previous face was Fira Sans
///   *Medium* (500). That one step is what makes the HUD read thinner at the
///   7.5–9.5 px sizes it draws at.
///
/// ⚠️ **The axis is 400…700** — Arimo has no cut below Regular, so
/// `FontWeight::LIGHT`/`THIN` clamp to 400. Regular is as thin as this face
/// goes; a genuinely light Arial-metric face does not exist under OFL.
///
/// ⚠️ **Latin only.** Arimo carries no Hangul, so a server whose item and NPC
/// names are Korean wants `fonts.pk2_faces: true` (see [`FontRole`]).
///
/// History: the face before Fira was `assets/fonts/9.ttf`, removed in #637 —
/// its `name` table carried an all-rights-reserved MorrisDesign notice
/// (UTF-16BE, so a raw-ASCII grep does not find it) with no license entry, and
/// git-tracking it in a GPL-3.0 repository was the exposure, independent of
/// whether it was used.
pub const BUNDLED_FALLBACK_FACE: &str = "fonts/Arimo-Variable.ttf";

#[derive(AssetCollection, Resource)]
#[allow(dead_code)]
pub struct FontAssets {
    // These four start on the bundled OFL face so text always renders, and are
    // swapped to the user's PK2 face by `apply_pk2_face` when that is asked
    // for. The slot names mirror the original's `FontIndex` values; the index
    // -> face mapping itself is [U], so all four currently resolve to the same
    // role rather than to an invented table.
    #[asset(path = "fonts/Arimo-Variable.ttf")]
    pub one: Handle<Font>,
    #[asset(path = "fonts/Arimo-Variable.ttf")]
    pub two: Handle<Font>,
    #[asset(path = "fonts/Arimo-Variable.ttf")]
    pub three: Handle<Font>,
    #[asset(path = "fonts/Arimo-Variable.ttf")]
    pub nine: Handle<Font>,
}

/// Swap the UI slots onto the user's PK2 face, but only once it has really
/// loaded — and only when `fonts.pk2_faces` asks for it, which is off by
/// default (see [`FontSettings`](crate::plugins::config::fonts::FontSettings)).
///
/// Idea: pointing the slots straight at `media://` would blank every label on
/// a tree with no PK2 (or with an archive that spells the file differently),
/// so the bundled OFL face stays in place until the load *succeeds*. Failure
/// is logged once and the fallback keeps rendering.
fn apply_pk2_face(
    // Optional: `SroAssetStructsPlugin` is built headless in this module's own
    // tests, where no `ClientConfig` exists. A missing config means "not
    // configured yet", not "use the bundled face forever".
    config: Option<Res<ClientConfig>>,
    asset_server: Res<AssetServer>,
    fonts: Option<ResMut<FontAssets>>,
    mut pending: Local<Option<Handle<Font>>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Some(config) = config else {
        return;
    };
    if !config.fonts.pk2_faces {
        *done = true;
        return;
    }
    let handle = pending
        .get_or_insert_with(|| asset_server.load(FontRole::DefaultUi.path()))
        .clone();
    let state = asset_server.load_state(&handle);
    if state.is_failed() {
        warn!(
            "fonts: {} did not load, keeping the bundled fallback ({})",
            FontRole::DefaultUi.path(),
            BUNDLED_FALLBACK_FACE
        );
        *done = true;
        return;
    }
    if !state.is_loaded() {
        return;
    }
    let Some(mut fonts) = fonts else {
        return;
    };
    fonts.one = handle.clone();
    fonts.two = handle.clone();
    fonts.three = handle.clone();
    fonts.nine = handle;
    *done = true;
    info!("fonts: UI face loaded from the PK2");
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use bytes::Buf;

    use super::*;

    /// #637: the repository must ship only fonts it may redistribute. The
    /// bundled face is the vendored OFL Arimo, an Arial-metric substitute; the
    /// original's own faces come from the user's PK2, never from this tree.
    #[test]
    fn the_bundled_face_is_the_ofl_one() {
        assert_eq!(BUNDLED_FALLBACK_FACE, "fonts/Arimo-Variable.ttf");
        assert!(!BUNDLED_FALLBACK_FACE.contains("9.ttf"));
    }

    /// The bundled face must actually be on disk, be a TrueType file, and be
    /// **variable** — the whole UI now takes both its body weight and its bold
    /// emphasis from one file's `wght` axis, so a static face here would
    /// silently flatten every bold label back to regular.
    #[test]
    fn the_bundled_face_is_a_variable_truetype_on_disk() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../assets/fonts/Arimo-Variable.ttf"
        );
        let bytes = std::fs::read(path).expect("the bundled UI face is vendored");
        let mut cursor = Cursor::new(&bytes);
        // sfnt header: version, then the table count
        let version = cursor.get_u32();
        assert_eq!(version, 0x0001_0000, "not a TrueType outline font");
        let table_count = cursor.get_u16() as usize;
        cursor.advance(6);
        let mut tags = Vec::with_capacity(table_count);
        for _ in 0..table_count {
            let mut tag = [0u8; 4];
            cursor.copy_to_slice(&mut tag);
            cursor.advance(12);
            tags.push(tag);
        }
        assert!(
            tags.contains(b"fvar"),
            "no `fvar` table: the face is not variable, so FontWeight cannot \
             synthesize the bold the tooltip and nameplates ask for"
        );
        // and its licence ships beside it, as OFL 1.1 requires
        assert!(
            std::path::Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../assets/fonts/Arimo-OFL.txt"
            ))
            .exists(),
            "the OFL text must ship with the face"
        );
    }

    /// The three faces are addressed by role, read off the archive's own file
    /// names. `FontIndex` -> face stays [U]
    /// (`docs/re/ui/localization-and-fonts.md` §9-U1), so this mapping is the
    /// single place a later resolution has to touch.
    #[test]
    fn font_roles_point_into_the_users_pk2() {
        assert_eq!(FontRole::DefaultUi.path(), "media://fonts/기본서체.ttf");
        assert_eq!(FontRole::Latin.path(), "media://fonts/영문서체.ttf");
        assert_eq!(FontRole::Chat.path(), "media://fonts/채팅서체.ttf");
        for role in [FontRole::DefaultUi, FontRole::Latin, FontRole::Chat] {
            assert!(
                role.path().starts_with("media://fonts/"),
                "{role:?} must resolve inside the user's archive"
            );
        }
    }

    /// #477: `InterfaceText` and its loader were registered but commented out,
    /// so the resinfo loader was dead code no `AssetServer::load` could reach.
    /// This asserts the wire is live — and, in the same breath, that turning it
    /// on did not displace `Textdata`, which claims the same `txt` extension.
    #[test]
    fn resinfo_and_textdata_asset_types_are_both_registered() {
        let mut app = App::new();
        // #665: `TaskPoolPlugin` first, then `AssetPlugin` — the order
        // `MinimalPlugins`/`DefaultPlugins` use. `AssetPlugin` reaches the
        // process-global `IoTaskPool` `OnceLock` as soon as anything is loaded
        // *by path*, and that lock is shared by every test in the binary, so a
        // fixture without the pool plugin is green by test order rather than by
        // construction. Copy this pair, not just the asset plugin.
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .add_plugins(SroAssetStructsPlugin);

        assert!(
            app.world()
                .get_resource::<bevy::asset::Assets<InterfaceText>>()
                .is_some(),
            "InterfaceText is not registered — the resinfo loader is dead again"
        );
        assert!(
            app.world()
                .get_resource::<bevy::asset::Assets<Textdata>>()
                .is_some(),
            "Textdata registration was displaced by the resinfo loader"
        );
    }

    /// #665: the app-test fixtures build `AssetPlugin` directly instead of
    /// `MinimalPlugins`, and `AssetPlugin` alone does not create the global
    /// `IoTaskPool` — `AssetServer::load` (load *by path*) then panics unless
    /// some earlier test in the same binary happened to install the pool. This
    /// drives exactly that path, so the fixture pairing is asserted by
    /// behaviour rather than by grep.
    #[test]
    fn asset_fixture_can_load_by_path() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ))
        .add_plugins(SroAssetStructsPlugin);

        assert!(
            app.is_plugin_added::<bevy::app::TaskPoolPlugin>(),
            "the fixture dropped TaskPoolPlugin — AssetServer::load would be              order-dependent again"
        );

        // The file need not exist: a missing asset resolves to a failed load,
        // whereas a missing IoTaskPool panics inside `load` itself.
        let _handle: Handle<Textdata> = app
            .world()
            .resource::<bevy::asset::AssetServer>()
            .load("665-does-not-exist.txt");
        app.update();
    }

    #[test]
    pub fn test_read_str_and_jump() {
        let bytes: [u8; 30] = [
            0, 0, 0, 22, 84, 104, 105, 115, 32, 105, 115, 32, 97, 32, 116, 101, 115, 116, 32, 115,
            116, 114, 105, 110, 103, 46, 0, 0, 0, 5,
        ];

        let mut cursor = Cursor::new(bytes.as_slice());
        let length = cursor.get_i32(); // 22

        assert_eq!(4, cursor.position());
        assert_eq!(22, length);

        let str = read_str_and_jump(&mut cursor, length as u64);
        assert_eq!(str, "This is a test string.");
        assert_eq!(cursor.position(), 26);

        let i = cursor.get_u32();
        assert_eq!(i, 5);
    }

    #[test]
    pub fn test_read_str() {
        let bytes: [u8; 22] = [
            84, 104, 105, 115, 32, 105, 115, 32, 97, 32, 116, 101, 115, 116, 32, 115, 116, 114,
            105, 110, 103, 46,
        ];
        let str = read_str(&bytes);
        assert_eq!(str, "This is a test string.");
    }
}

/*
asiaminor_field.ogg		forgotten_ghost.ogg
battlearena_1.ogg		forgotten_togui.ogg
centralasia_field.ogg		fortress_war.ogg
centralasia_town.ogg		hotan_field.ogg
donwhang_dungeon.ogg		jangan_field.ogg
donwhang_field.ogg		jangan_town.ogg
donwhang_town.ogg		jinsi_dungeon.ogg
easterneurope_field.ogg		karakoram_field.ogg
easterneurope_town.ogg		maintheme_cut.ogg
egypt_dungeon_pharaoh.ogg	mtrok_field.ogg
egypt_dungeon_temple.ogg	roc_battle.ogg
egypt_field_delta.ogg		roc_bgm_total.ogg
egypt_field_kingsvalley.ogg	roc_entrance.ogg
egypt_field_stormdesert.ogg	shiningstar.ogg
egypt_town_alexandria1.ogg	silkroad_돈황석굴.ogg
egypt_town_alexandria2.ogg	taklamakan_field.ogg
event_carol_01.ogg		돈황.ogg
event_carol_02.ogg		장안.ogg
event_carol_03.ogg		돈황필드.ogg
event_carol_04.ogg		장안필드.ogg
event_festival.ogg		호탄필드.ogg
event_ghost.ogg			카라코람필드.ogg
forgotten_flame.ogg*/
