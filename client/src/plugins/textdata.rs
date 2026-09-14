use bevy::app::App;
use bevy::asset::{AssetServer, Assets};
use bevy::log::{debug, info};
use bevy::prelude::{
    AssetEvent, Commands, Handle, MessageReader, OnEnter, Plugin, Res, ResMut, Resource, Update,
};

use crate::assets::resinfo::item_rare::{ItemRareTable, RareEffect};
use crate::assets::textdata::actionwnddata::{ActionCommand, ActionWndData};
use crate::assets::textdata::characterdata::{CharacterData, CharacterDataRow};
use crate::assets::textdata::collectionbook::CollectionBookTable;
use crate::assets::textdata::dungeoninfo::{DungeonEntry, DungeonInfo};
use crate::assets::textdata::effectsound::{EffectSound, EffectSoundTable, SoundAddress};
use crate::assets::textdata::gameguide::{GuideIndex, GuideNode};
use crate::assets::textdata::itemdata::{ItemData, ItemDataRow};
use crate::assets::textdata::leveldata::LevelData;
use crate::assets::textdata::magicoption::{MagicOptionData, MagicOptionInfo};
use crate::assets::textdata::masterydata::{MasteryData, MasteryInfo};
use crate::assets::textdata::names::TextdataNames;
use crate::assets::textdata::npcchat::{NpcChat, NpcChatEntry};
use crate::assets::textdata::questreward::{QuestRewardItem, QuestRewardItems, QuestRewardModes};
use crate::assets::textdata::shops::{ShopLayout, ShopTable};
use crate::assets::textdata::skilldata::{SkillData, SkillDataRow};
use crate::assets::textdata::skilleffect::{SkillEffectTable, SkillEntry};
use crate::assets::textdata::skillgroup::SkillGroupTable;
use crate::assets::textdata::teleport::{TeleportInfo, TeleportLink, TeleportTable};
use crate::assets::textdata::uisystem::UiSystemText;
use crate::assets::textdata::worldmap::WorldMapTable;
use crate::assets::textdata::zonenames::ZoneNames;
use crate::assets::textdata::zonesound::{ZoneSound, ZoneSoundTable};
use crate::assets::textdata::Textdata;
use crate::scenes::SceneState;

#[derive(Default)]
pub struct TextdataPlugin;

#[derive(Resource, Default)]
#[allow(dead_code)]
struct TextdataHandles(Vec<Handle<Textdata>>);

#[derive(Resource, Default)]
pub struct ClientCharacterData(Option<CharacterData>);

impl ClientCharacterData {
    /// Test-only constructor: builds the resource straight from a table so
    /// data-shape regressions (e.g. #643, a race silently missing from the
    /// race board) can be pinned to real characterdata rows in a unit test.
    #[cfg(test)]
    pub(crate) fn from_table(data: CharacterData) -> Self {
        Self(Some(data))
    }

    pub fn get(&self, id: &i32) -> Option<&CharacterDataRow> {
        match &self.0 {
            Some(cd) => cd.get(id),
            None => None,
        }
    }

    /// The whole table, for consumers that scan it (monster pickers).
    pub fn data(&self) -> Option<&CharacterData> {
        self.0.as_ref()
    }

    /// The `.bsr` model path for a row, following the `OrgObjCode` reference
    /// when the row has no own model (`xxx`): summon clones/variants reuse
    /// their base object's model (e.g. the water ghost slave renders the
    /// water ghost model). Bounded chase guards against a reference cycle.
    pub fn model_path(&self, row: &CharacterDataRow) -> Option<String> {
        if let Some(path) = row.resource_path() {
            return Some(path);
        }
        let Some(cd) = &self.0 else {
            return None;
        };
        let mut base_code = row.org_obj_code()?.to_string();
        for _ in 0..4 {
            let base = cd.values().find(|r| r.code_name() == &base_code)?;
            if let Some(path) = base.resource_path() {
                return Some(path);
            }
            base_code = base.org_obj_code()?.to_string();
        }
        None
    }
}

#[derive(Resource, Default)]
pub struct ClientItemData(Option<ItemData>);

impl ClientItemData {
    /// Seed the resource directly, bypassing the asset pipeline (tests only).
    #[cfg(test)]
    pub(crate) fn from_data(data: ItemData) -> Self {
        Self(Some(data))
    }

    pub fn get(&self, id: &i32) -> Option<&ItemDataRow> {
        match &self.0 {
            Some(id_data) => id_data.get(id),
            None => None,
        }
    }

    /// The full table (for pickers/scans), `None` until loaded.
    pub fn data(&self) -> Option<&ItemData> {
        self.0.as_ref()
    }
}

/// Lets the packets crate resolve an item's class from the client-held
/// itemdata (same mapping as `net::entity_spawn::TextdataResolver`), so
/// resolver-driven item parsing (inventory pickups) works with just the item
/// table.
impl packets::agent::character_data::ItemClassResolver for ClientItemData {
    fn item_class(&self, ref_id: u32) -> packets::agent::character_data::ItemClass {
        use packets::agent::character_data::ItemClass;
        match self.get(&(ref_id as i32)).and_then(|i| i.type_ids()) {
            Some((3, 1, _, _)) => ItemClass::Equipment,
            Some((3, 2, tid3, tid4)) => ItemClass::Container { tid3, tid4 },
            Some((3, 3, tid3, tid4)) => ItemClass::Expendable { tid3, tid4 },
            _ => ItemClass::Unknown,
        }
    }
}

#[derive(Resource, Default)]
pub struct ClientSkillData(Option<SkillData>);

impl ClientSkillData {
    pub fn get(&self, id: &i32) -> Option<&SkillDataRow> {
        match &self.0 {
            Some(sd) => sd.get(id),
            None => None,
        }
    }

    /// Whether the table has arrived (distinguishes "unknown skill id" from
    /// "table not loaded yet" for consumers that want to retry).
    pub fn is_loaded(&self) -> bool {
        self.0.is_some()
    }

    /// The whole table, for consumers that index/scan it (skill book rules).
    pub fn data(&self) -> Option<&SkillData> {
        self.0.as_ref()
    }
}

#[derive(Resource, Default)]
pub struct ClientSkillEffects(Option<SkillEffectTable>);

impl ClientSkillEffects {
    /// Animation binding + effect emissions for a skill's basic-group
    /// codename (skilldata col 5), e.g. `SKILL_CH_SWORD_SMASH_A`.
    pub fn get(&self, codename: &str) -> Option<&SkillEntry> {
        let table = self.0.as_ref()?;
        table
            .by_codename
            .get(codename)
            .map(|&idx| &table.skills[idx])
    }

    /// The unique `.bsr` MODEL paths referenced by skill emissions (bow
    /// arrows). Preloaded so a cast's short-lived projectile doesn't lose
    /// the async load race and silently render nothing.
    pub fn model_paths(&self) -> Vec<String> {
        let Some(table) = self.0.as_ref() else {
            return Vec::new();
        };
        let mut paths: Vec<String> = table
            .skills
            .iter()
            .flat_map(|s| s.emissions.iter())
            .filter(|e| e.efp_path.ends_with(".bsr"))
            .map(|e| e.efp_path.clone())
            .collect();
        paths.sort_unstable();
        paths.dedup();
        paths
    }
}

#[derive(Resource, Default)]
pub struct ClientSkillGroups(Option<SkillGroupTable>);

impl ClientSkillGroups {
    /// Display row (skillgroup.txt Row column) of a skill series, matched by
    /// the branch's icon concept occurring in `basic_group`. `None` when no
    /// branch matches or the table has not loaded yet.
    pub fn row_for(&self, mastery: u32, basic_group: &str) -> Option<u32> {
        self.0
            .as_ref()
            .and_then(|t| t.row_for(mastery, basic_group))
    }

    /// The branch's authoritative icon asset path (`media://icon/skillgroup/
    /// <race>/pack_<concept>.ddj`, skillgroup.txt col 6), matched the same way
    /// as [`Self::row_for`]. `None` falls back to the root skill's own icon.
    pub fn icon_for(&self, mastery: u32, basic_group: &str) -> Option<String> {
        self.0
            .as_ref()
            .and_then(|t| t.branch_for(mastery, basic_group))
            .map(|branch| format!("media://{}", branch.icon))
    }
}

#[derive(Resource, Default)]
pub struct ClientMasteryData(Option<MasteryData>);

impl ClientMasteryData {
    pub fn get(&self, id: u32) -> Option<&MasteryInfo> {
        self.0.as_ref().and_then(|data| data.get(&id))
    }

    /// All masteries, unordered; callers sort by (group, tab) for the window.
    pub fn iter(&self) -> impl Iterator<Item = &MasteryInfo> {
        self.0.iter().flat_map(|data| data.values())
    }
}

#[derive(Resource, Default)]
pub struct ClientLevelData(Option<LevelData>);

impl ClientLevelData {
    /// Seed the resource directly, bypassing the asset pipeline (tests only).
    #[cfg(test)]
    pub(crate) fn from_data(data: LevelData) -> Self {
        Self(Some(data))
    }

    /// Exp required to complete the given level, from leveldata.txt.
    pub fn max_exp(&self, level: u8) -> Option<u64> {
        self.0.as_ref().and_then(|data| data.max_exp(level))
    }

    /// SP cost of raising a mastery to `target_level` (leveldata col 2).
    pub fn mastery_sp_cost(&self, target_level: u8) -> Option<u32> {
        self.0
            .as_ref()
            .and_then(|data| data.mastery_sp_cost(target_level))
    }
}

#[derive(Resource, Default)]
pub struct ClientTextNames(Option<TextdataNames>);

impl ClientTextNames {
    /// Localized display string for an `SN_*` key, from the textdataname
    /// tables (item/mob/npc names).
    pub fn name(&self, key: &str) -> Option<&str> {
        self.0.as_ref().and_then(|names| names.get(key))
    }
}

#[derive(Resource, Default)]
pub struct ClientMagicOptions(Option<MagicOptionData>);

impl ClientMagicOptions {
    /// Definition of an item magic option by its id (a `MagicParam.kind`).
    pub fn get(&self, id: u32) -> Option<&MagicOptionInfo> {
        self.0.as_ref().and_then(|data| data.get(&id))
    }
}

#[derive(Resource, Default)]
pub struct ClientRareEffects(std::collections::HashMap<String, Vec<RareEffect>>);

impl ClientRareEffects {
    /// The rare auras for an item by its code name — the `ItemRare.txt`
    /// (raretype) aura followed by the `ItemOptionEfp.txt` (enchant) one, both
    /// attached when the item is worn.
    pub fn get(&self, code: &str) -> &[RareEffect] {
        self.0.get(code).map_or(&[], Vec::as_slice)
    }
}

/// Keeps the rare-aura table handles alive (ItemRare.txt, ItemOptionEfp.txt).
#[derive(Resource, Default)]
struct RareEffectHandles(Vec<Handle<ItemRareTable>>);

/// The action window's command table (`actionwnddata.txt`). Its slots are keyed
/// on `(group, index)` — the layout's `CommandID` is not unique (see
/// `assets/textdata/actionwnddata.rs`).
#[derive(Resource, Default)]
pub struct ClientActionCommands(Option<ActionWndData>);

impl ClientActionCommands {
    /// The live command in `index` of `group`, or `None` for an unassigned slot.
    pub fn slot(&self, group: u8, index: u8) -> Option<&ActionCommand> {
        self.0.as_ref().and_then(|table| table.slot(group, index))
    }
}

/// The talisman collection book (`collectionbook_theme.txt` + its item
/// sibling) — the whole book's content, entirely client-side.
#[derive(Resource, Default)]
pub struct ClientCollectionBook(Option<CollectionBookTable>);

impl ClientCollectionBook {
    pub fn table(&self) -> Option<&CollectionBookTable> {
        self.0.as_ref()
    }
}

#[derive(Resource, Default)]
pub struct ClientUiStrings(Option<UiSystemText>);

impl ClientUiStrings {
    /// Localized UI string for a `UIIT_*`-style key, from textuisystem.txt.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.as_ref().and_then(|strings| strings.get(key))
    }

    /// Lookup with a hardcoded fallback for when the table is missing the key
    /// (or has not loaded, e.g. in offline preview scenes). Fallback hits on a
    /// loaded table are logged so stale keys surface during playtests.
    pub fn get_or<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        match self.get(key) {
            Some(value) => value,
            None => {
                if self.0.is_some() {
                    bevy::log::debug!("textuisystem: no entry for {key}, using \"{fallback}\"");
                }
                fallback
            }
        }
    }

    /// [`Self::get_or`] with the row's markup resolved, for the rows that carry
    /// it — see [`plain_text`]. Use this for message bodies and notices; plain
    /// labels are unaffected either way.
    pub fn get_plain_or(&self, key: &str, fallback: &str) -> String {
        plain_text(self.get_or(key, fallback))
    }
}

/// Flatten one textuisystem row's markup into text a Bevy `Text` node can show.
///
/// Idea: a large minority of textuisystem's rows are authored for the original's
/// `CIFPML` rich-text control, not for a plain label — they carry `<sml2>`,
/// `<br>` and `<font …>` tags (`docs/re/ui/help-tooltip-widget.md`: 93 rows use
/// the `sml2` dialect). A plain `Text` node renders those tags **literally**, so
/// the user sees the markup. We do not implement CIFPML; the honest reduction is
/// to keep the authored line breaks (`<br>`) and drop the rest of the markup,
/// which leaves markup-free rows byte-identical.
///
/// Deliberately not a general HTML parser: the dialect is a closed set of tags
/// in shipped data, and anything unterminated is passed through untouched rather
/// than swallowed.
pub fn plain_text(pml: &str) -> String {
    let mut out = String::with_capacity(pml.len());
    let mut rest = pml;
    while let Some(open) = rest.find('<') {
        let Some(len) = rest[open..].find('>') else {
            break;
        };
        out.push_str(&rest[..open]);
        if rest[open + 1..open + len]
            .trim_end_matches('/')
            .eq_ignore_ascii_case("br")
        {
            out.push('\n');
        }
        rest = &rest[open + len + 1..];
    }
    out.push_str(rest);
    out
}

#[derive(Resource, Default)]
pub struct ClientShops(Option<ShopTable>);

impl ClientShops {
    /// The denormalized shop inventory of an NPC, by its characterdata
    /// codename.
    pub fn shop_for_npc(&self, npc_codename: &str) -> Option<&ShopLayout> {
        self.0.as_ref().and_then(|table| table.get(npc_codename))
    }
}

/// Itemdata codename → ref id, built once when the item table arrives (the
/// shop chain references items by codename; scanning 40k rows per lookup
/// would be silly).
#[derive(Resource, Default)]
pub struct ClientItemIndex(std::collections::HashMap<String, i32>);

impl ClientItemIndex {
    /// Seed the index directly, bypassing the asset pipeline (tests only).
    #[cfg(test)]
    pub(crate) fn from_pairs<I: IntoIterator<Item = (String, i32)>>(pairs: I) -> Self {
        Self(pairs.into_iter().collect())
    }

    pub fn id(&self, codename: &str) -> Option<i32> {
        self.0.get(codename).copied()
    }
}

/// Characterdata codename → ref id, the mirror of [`ClientItemIndex`] for the
/// character table.
///
/// Built for the same reason: the GM commands name a monster by codename
/// (`/loadmonster`, `/zoe2`), and without an index that is a linear scan of
/// ~14k rows per lookup — which is what `ClientCharacterData::model_path`
/// still does for its one-off resolve.
#[derive(Resource, Default)]
pub struct ClientCharacterIndex(std::collections::HashMap<String, i32>);

impl ClientCharacterIndex {
    /// Seed the index directly, bypassing the asset pipeline (tests only).
    #[cfg(test)]
    pub(crate) fn from_pairs<I: IntoIterator<Item = (String, i32)>>(pairs: I) -> Self {
        Self(pairs.into_iter().collect())
    }

    pub fn id(&self, codename: &str) -> Option<i32> {
        self.0.get(codename).copied()
    }
}

#[derive(Resource, Default)]
pub struct ClientWorldMap(Option<WorldMapTable>);

impl ClientWorldMap {
    pub fn table(&self) -> Option<&WorldMapTable> {
        self.0.as_ref()
    }
}

/// `regioninfo.txt` x `effectenvsnd.txt` — which zone a region id belongs to
/// and what that zone sounds like (EP-22.1). Playback lives elsewhere.
#[derive(Resource, Default)]
pub struct ClientZoneSounds(Option<ZoneSoundTable>);

// Consumed by the playback half (#771).
#[allow(dead_code)]
impl ClientZoneSounds {
    /// Zone at a sector-local position; `RECT` claims beat the sector's `ALL`
    /// background (`assets::textdata::zonesound`).
    pub fn zone_at(&self, region_id: u16, local_x: f32, local_z: f32) -> Option<&ZoneSound> {
        self.0
            .as_ref()
            .and_then(|t| t.zone_at(region_id, local_x, local_z))
    }

    /// Zone owning a whole sector, for callers that only have a region id.
    pub fn zone_for_region(&self, region_id: u16) -> Option<&ZoneSound> {
        self.0.as_ref().and_then(|t| t.zone_for_region(region_id))
    }
}

/// `effectsound.txt` — the SFX registry (EP-22.2): an address tuple resolves
/// to a `.wav` under `Data.pk2:prim/snd/` plus the row's own volume, so a call
/// site plays what the data says instead of a hard-coded path.
#[derive(Resource, Default)]
pub struct ClientEffectSounds(Option<EffectSoundTable>);

impl ClientEffectSounds {
    /// First row registered at an address, or `None` while the table is still
    /// loading / when the data does not name that sound.
    pub fn first(&self, address: &SoundAddress) -> Option<&EffectSound> {
        self.0.as_ref().and_then(|t| t.first(address))
    }

    /// Every variant at an address, in file order (the equip/skill wiring of
    /// the follow-up tickets picks among them).
    pub fn sounds(&self, address: &SoundAddress) -> &[EffectSound] {
        self.0.as_ref().map(|t| t.sounds(address)).unwrap_or(&[])
    }
}

#[derive(Resource, Default)]
pub struct ClientDungeonInfo(Option<DungeonInfo>);

impl ClientDungeonInfo {
    /// Dungeon entry for a dungeon-flagged region id (`0x8000 | id`).
    pub fn by_region(&self, region_id: u16) -> Option<&DungeonEntry> {
        self.0.as_ref().and_then(|info| info.by_region(region_id))
    }

    /// All dungeons, ordered by id (for pickers).
    pub fn entries(&self) -> impl Iterator<Item = &DungeonEntry> {
        self.0.iter().flat_map(|info| info.entries())
    }
}

#[derive(Resource, Default)]
pub struct ClientTeleport(Option<TeleportTable>);

impl ClientTeleport {
    /// The teleporter id + outgoing links of the teleporter owned by an NPC/
    /// building characterdata ref id.
    pub fn destinations(&self, owner_ref: i32) -> Option<(u32, &[TeleportLink])> {
        self.0
            .as_ref()
            .and_then(|table| table.destinations(owner_ref))
    }

    /// Display info of a teleporter id (destination naming).
    pub fn info(&self, id: u32) -> Option<&TeleportInfo> {
        self.0.as_ref().and_then(|table| table.info.get(&id))
    }

    /// Display key (`SN_NPC_*_GATE`) of a gate building's ref id — gate refs
    /// have no characterdata rows, so the spawn path names them from here.
    pub fn owner_name_key(&self, owner_ref: i32) -> Option<&str> {
        self.0
            .as_ref()
            .and_then(|table| table.buildings.get(&owner_ref))
            .map(|building| building.name_key.as_str())
    }

    /// Codename (`STORE_CH_GATE`) of a gate building's ref id — the dialog's
    /// npcchat/speech lookup key when characterdata has no row.
    pub fn owner_codename(&self, owner_ref: i32) -> Option<&str> {
        self.0
            .as_ref()
            .and_then(|table| table.buildings.get(&owner_ref))
            .map(|building| building.codename.as_str())
    }

    /// The whole table, for consumers that scan rows (the offline dungeon
    /// gates walk every owner-ref-0 circle gate).
    pub fn table(&self) -> Option<&TeleportTable> {
        self.0.as_ref()
    }

    /// Whether a spawn-record ref id is a gate building (teleportbuilding
    /// row) — the spawn resolver's structure test.
    pub fn is_gate_ref(&self, ref_id: i32) -> bool {
        self.0
            .as_ref()
            .is_some_and(|table| table.buildings.contains_key(&ref_id))
    }
}

#[derive(Resource, Default)]
pub struct ClientSpeechText(Option<UiSystemText>);

impl ClientSpeechText {
    /// NPC/quest speech string for an `SN_*` key, from
    /// `textquest_speech&name.txt`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.as_ref().and_then(|strings| strings.get(key))
    }
}

#[derive(Resource, Default)]
pub struct ClientNpcChat(Option<NpcChat>);

impl ClientNpcChat {
    /// The dialog speech string ids of an NPC, by its characterdata codename.
    pub fn get(&self, codename: &str) -> Option<&NpcChatEntry> {
        self.0.as_ref().and_then(|chat| chat.get(codename))
    }
}

/// Quest reward tables (`refqusetreward.txt` + `refquestrewarditems.txt`).
#[derive(Resource, Default)]
pub struct ClientQuestRewards(Option<(QuestRewardModes, QuestRewardItems)>);

impl ClientQuestRewards {
    /// `true` when the quest's reward is a choose-one pick (`SelectionCnt`).
    pub fn choose_one(&self, quest: u32) -> bool {
        self.0
            .as_ref()
            .is_some_and(|(modes, _)| modes.choose_one(quest))
    }

    /// The quest's reward rows: candidates in pick-one mode, all granted
    /// otherwise.
    pub fn items(&self, quest: u32) -> &[QuestRewardItem] {
        match &self.0 {
            Some((_, items)) => items.items(quest),
            None => &[],
        }
    }
}

/// The in-game help book (#575): the `gameguidedata.txt` tree and the
/// `texthelp.txt` bodies behind it. Two files, one resource, because a guide
/// node without its body is not usable on its own.
#[derive(Resource, Default)]
pub struct ClientGameGuide {
    index: Option<GuideIndex>,
    bodies: Option<UiSystemText>,
}

impl ClientGameGuide {
    /// The guide tree in file order, empty until `gameguidedata.txt` loads.
    pub fn nodes(&self) -> &[GuideNode] {
        self.index
            .as_ref()
            .map(|i| i.nodes.as_slice())
            .unwrap_or(&[])
    }

    /// The page body for a node, with the CIFPML markup reduced the same way
    /// every other markup-carrying table is ([`plain_text`]).
    pub fn body(&self, node: &GuideNode) -> Option<String> {
        let key = node.body_key.as_deref()?;
        self.bodies.as_ref()?.get(key).map(plain_text)
    }

    /// The drawer label for a node: the `SRO_GGW_MENU_*` string when
    /// `texthelp.txt` carries one, else the row's own authored title — which
    /// is Korean, and is all the data has for a row whose menu key is missing.
    /// Falling back to the title rather than to an empty row keeps an
    /// unresolved key visible instead of silently deleting a navigation entry.
    pub fn menu_label(&self, node: &GuideNode) -> String {
        node.menu_key
            .as_deref()
            .and_then(|key| self.bodies.as_ref()?.get(key))
            .map(plain_text)
            .unwrap_or_else(|| node.title.clone())
    }
}

#[derive(Resource, Default)]
pub struct ClientZoneNames(Option<ZoneNames>);

impl ClientZoneNames {
    /// Area name for a region id, from textzonename.txt.
    pub fn name(&self, region: u16) -> Option<&str> {
        self.0.as_ref().and_then(|names| names.name(region))
    }
}

impl Plugin for TextdataPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TextdataHandles>()
            .init_resource::<ClientCharacterData>()
            .init_resource::<ClientItemData>()
            .init_resource::<ClientSkillData>()
            .init_resource::<ClientSkillEffects>()
            .init_resource::<ClientMasteryData>()
            .init_resource::<ClientSkillGroups>()
            .init_resource::<ClientLevelData>()
            .init_resource::<ClientTextNames>()
            .init_resource::<ClientMagicOptions>()
            .init_resource::<ClientRareEffects>()
            .init_resource::<ClientZoneNames>()
            .init_resource::<ClientGameGuide>()
            .init_resource::<ClientActionCommands>()
            .init_resource::<ClientCollectionBook>()
            .init_resource::<ClientQuestRewards>()
            .init_resource::<ClientUiStrings>()
            .init_resource::<ClientSpeechText>()
            .init_resource::<ClientNpcChat>()
            .init_resource::<ClientShops>()
            .init_resource::<ClientItemIndex>()
            .init_resource::<ClientCharacterIndex>()
            .init_resource::<ClientTeleport>()
            .init_resource::<ClientWorldMap>()
            .init_resource::<ClientDungeonInfo>()
            .init_resource::<ClientZoneSounds>()
            .init_resource::<ClientEffectSounds>()
            .add_systems(OnEnter(SceneState::Loading), load_textdata)
            .add_systems(
                Update,
                (
                    add_resource_when_textdata_loaded,
                    add_rare_effects_when_loaded,
                ),
            );
    }
}

fn load_textdata(asset_server: Res<AssetServer>, mut commands: Commands) {
    let handles = vec![
        asset_server.load("media://server_dep/silkroad/textdata/characterdata.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/itemdata.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/skilldata.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/skilleffect.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/skillmasterydata.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/skillgroup.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/leveldata.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/textzonename.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/refqusetreward.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/textdataname.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/magicoption.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/textuisystem.txt"),
        // the in-game help book: the tree and its bodies (#575)
        asset_server.load("media://server_dep/silkroad/textdata/gameguidedata.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/texthelp.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/actionwnddata.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/collectionbook_theme.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/textquest_speech&name.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/npcchat.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/refshopgroup.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/teleportdata.txt"),
        asset_server.load("media://server_dep/silkroad/textdata/worldmap_mapinfo.txt"),
        // Dungeon id → .dof path table; lives in Data.pk2, not Media.pk2.
        asset_server.load("data://dungeon/dungeoninfo.txt"),
        // Zone sound tables: regioninfo.txt pulls in effectenvsnd.txt (EP-22.1).
        // regioninfo is the one table that is NOT in Media's server_dep tree —
        // it sits at the root of Data.pk2 as `RegionInfo.txt` (the archive
        // lookup lowercases, so the request need not match its casing). Its
        // effectenvsnd sibling stays in Media, which is where `read_sibling`
        // looks either way, so the join still resolves across both archives.
        asset_server.load("data://regioninfo.txt"),
        // The SFX registry (EP-22.2).
        asset_server.load("media://server_dep/silkroad/textdata/effectsound.txt"),
    ];
    commands.insert_resource(TextdataHandles(handles));
    // Two aura layers per rare item: the raretype glow, then the enchant.
    commands.insert_resource(RareEffectHandles(vec![
        asset_server.load("media://resinfo/ItemRare.txt"),
        asset_server.load("media://resinfo/ItemOptionEfp.txt"),
    ]));
}

fn add_rare_effects_when_loaded(
    mut reader: MessageReader<AssetEvent<ItemRareTable>>,
    mut commands: Commands,
    handles: Option<Res<RareEffectHandles>>,
    tables: Res<Assets<ItemRareTable>>,
) {
    let Some(handles) = handles else { return };
    // Rebuild the merged map once any table arrives and all are ready, so each
    // item's auras stay in layer order (raretype, then enchant).
    if !reader.read().any(|e| matches!(e, AssetEvent::Added { .. })) {
        return;
    }
    let loaded: Vec<&ItemRareTable> = handles.0.iter().filter_map(|h| tables.get(h)).collect();
    if loaded.len() != handles.0.len() {
        return;
    }
    let mut merged: std::collections::HashMap<String, Vec<RareEffect>> = Default::default();
    for table in loaded {
        for (code, effects) in &table.0 {
            merged
                .entry(code.clone())
                .or_default()
                .extend(effects.iter().cloned());
        }
    }
    info!("loaded {} rare-item aura sets", merged.len());
    commands.insert_resource(ClientRareEffects(merged));
}

fn add_resource_when_textdata_loaded(
    mut reader: MessageReader<AssetEvent<Textdata>>,
    mut commands: Commands,
    mut guide: ResMut<ClientGameGuide>,
    textdata_assets: Res<Assets<Textdata>>,
) {
    for event in reader.read() {
        let textdata = match event {
            AssetEvent::Added { id } => textdata_assets.get(*id),
            _ => None,
        };

        if let Some(textdata) = textdata {
            info!("updated textdata");
            match textdata {
                Textdata::CharacterData(char_data) => {
                    let index = char_data
                        .0
                        .iter()
                        .map(|(id, row)| (row.code_name().clone(), *id))
                        .collect();
                    commands.insert_resource(ClientCharacterIndex(index));
                    commands.insert_resource(ClientCharacterData(Some(char_data.clone())))
                }
                Textdata::ItemData(item_data) => {
                    info!("loaded {} itemdata rows", item_data.len());
                    let index = item_data
                        .0
                        .iter()
                        .map(|(id, row)| (row.code_name().clone(), *id))
                        .collect();
                    commands.insert_resource(ClientItemIndex(index));
                    commands.insert_resource(ClientItemData(Some(item_data.clone())))
                }
                Textdata::LevelData(level_data) => {
                    commands.insert_resource(ClientLevelData(Some(level_data.clone())))
                }
                Textdata::QuestRewards(modes, items) => {
                    info!(
                        "loaded {} quest reward modes, {} quests with reward rows",
                        modes.0.len(),
                        items.0.len()
                    );
                    commands
                        .insert_resource(ClientQuestRewards(Some((modes.clone(), items.clone()))))
                }
                Textdata::ZoneNames(zone_names) => {
                    info!("loaded {} zone names", zone_names.0.len());
                    commands.insert_resource(ClientZoneNames(Some(zone_names.clone())))
                }
                Textdata::Names(names) => {
                    info!("loaded {} display names", names.0.len());
                    commands.insert_resource(ClientTextNames(Some(names.clone())))
                }
                Textdata::SkillData(skill_data) => {
                    info!("loaded {} skilldata rows", skill_data.len());
                    commands.insert_resource(ClientSkillData(Some(skill_data.clone())))
                }
                Textdata::SkillEffects(table) => {
                    info!("loaded {} skilleffect entries", table.skills.len());
                    commands.insert_resource(ClientSkillEffects(Some(table.clone())))
                }
                Textdata::MasteryData(masteries) => {
                    info!("loaded {} masteries", masteries.len());
                    commands.insert_resource(ClientMasteryData(Some(masteries.clone())))
                }
                Textdata::SkillGroups(groups) => {
                    info!("loaded {} skill-group branches", groups.len());
                    commands.insert_resource(ClientSkillGroups(Some(groups.clone())))
                }
                Textdata::MagicOption(magic_options) => {
                    info!("loaded {} magic-option rows", magic_options.len());
                    commands.insert_resource(ClientMagicOptions(Some(magic_options.clone())))
                }
                Textdata::CollectionBook(book) => {
                    info!(
                        "loaded {} collection themes, {} talismans",
                        book.themes.len(),
                        book.items.len()
                    );
                    commands.insert_resource(ClientCollectionBook(Some(book.clone())))
                }
                Textdata::ActionWnd(table) => {
                    info!("loaded {} action-window commands", table.0.len());
                    commands.insert_resource(ClientActionCommands(Some(table.clone())))
                }
                // The guide's two halves land independently and fill one
                // resource, so they mutate it rather than replacing it.
                Textdata::GameGuide(index) => {
                    info!("loaded {} game-guide nodes", index.nodes.len());
                    for mismatch in index.child_count_mismatches() {
                        debug!(
                            "gameguidedata: category {} states {} children, {} rows point at it",
                            mismatch.category, mismatch.stated, mismatch.actual
                        );
                    }
                    guide.index = Some(index.clone());
                }
                Textdata::GuideText(bodies) => {
                    info!("loaded {} game-guide bodies", bodies.0.len());
                    guide.bodies = Some(bodies.clone());
                }
                Textdata::UiSystem(strings) => {
                    info!("loaded {} UI strings", strings.0.len());
                    commands.insert_resource(ClientUiStrings(Some(strings.clone())))
                }
                Textdata::SpeechText(strings) => {
                    info!("loaded {} speech strings", strings.0.len());
                    commands.insert_resource(ClientSpeechText(Some(strings.clone())))
                }
                Textdata::NpcChat(chat) => {
                    info!("loaded {} npc chat entries", chat.0.len());
                    commands.insert_resource(ClientNpcChat(Some(chat.clone())))
                }
                Textdata::Shops(shops) => {
                    info!("loaded {} npc shops", shops.by_npc.len());
                    commands.insert_resource(ClientShops(Some(shops.clone())))
                }
                Textdata::Teleport(teleport) => {
                    info!(
                        "loaded {} teleporters ({} linked)",
                        teleport.info.len(),
                        teleport.links.len()
                    );
                    commands.insert_resource(ClientTeleport(Some(teleport.clone())))
                }
                Textdata::WorldMap(worldmap) => {
                    info!(
                        "loaded {} world maps, {} POIs",
                        worldmap.maps.len(),
                        worldmap.pois.len()
                    );
                    commands.insert_resource(ClientWorldMap(Some(worldmap.clone())))
                }
                Textdata::DungeonInfo(dungeons) => {
                    info!("loaded {} dungeon info rows", dungeons.0.len());
                    commands.insert_resource(ClientDungeonInfo(Some(dungeons.clone())))
                }
                Textdata::ZoneSounds(zone_sounds) => {
                    info!(
                        "loaded {} sound zones ({} with a BGM track)",
                        zone_sounds.zones().len(),
                        zone_sounds
                            .zones()
                            .iter()
                            .filter(|z| z.bgm.is_some())
                            .count()
                    );
                    if !zone_sounds.unmatched_zones.is_empty() {
                        debug!(
                            "zone sounds: {} zone names in only one of regioninfo/effectenvsnd: {}",
                            zone_sounds.unmatched_zones.len(),
                            zone_sounds.unmatched_zones.join(", ")
                        );
                    }
                    commands.insert_resource(ClientZoneSounds(Some(zone_sounds.clone())))
                }
                Textdata::EffectSounds(effect_sounds) => {
                    info!(
                        "loaded {} effect sound rows ({} addresses, {} mute)",
                        effect_sounds.row_count(),
                        effect_sounds.len(),
                        effect_sounds.mute_rows()
                    );
                    commands.insert_resource(ClientEffectSounds(Some(effect_sounds.clone())))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows :3969 and :1753 verbatim from the user's
    /// `media://server_dep/silkroad/textdata/textuisystem.txt` — 10 tab-separated
    /// columns with the English text last. The first is authored for the
    /// original's `CIFPML` control and carries markup; the second does not.
    const TEXTUISYSTEM_ROWS: &str = concat!(
        "1\tUIIT_STT_GLOBAL_AUTHENTICATION_NOTICE\t\t\t\t\t\t\t\t",
        "<sml2>To prevent auto creation,<br>please enter the number/text as it appears.</sml2>\r\n",
        "1\tUIIT_MSG_LOGOUT_REMAIN_TIME\t\t\t\t\t\t\t\t",
        "It will take %d seconds to close the game.\r\n",
    );

    fn strings() -> ClientUiStrings {
        ClientUiStrings(Some(
            crate::assets::textdata::uisystem::UiSystemText::parse(TEXTUISYSTEM_ROWS),
        ))
    }

    /// A markup row must reach a `Text` node as text, not as tags — this is the
    /// only markup handling we have (`docs/re/ui/help-tooltip-widget.md` §8-5:
    /// 93 rows use the `sml2` dialect).
    #[test]
    fn get_plain_or_resolves_markup_rows() {
        assert_eq!(
            strings().get_plain_or("UIIT_STT_GLOBAL_AUTHENTICATION_NOTICE", "fallback"),
            "To prevent auto creation,\nplease enter the number/text as it appears."
        );
    }

    /// ...and a markup-free row, a `%d` template and a fallback must all come
    /// through byte-identical, so adopting `get_plain_or` at a call site can
    /// never change a plain string.
    #[test]
    fn get_plain_or_leaves_plain_rows_and_fallbacks_untouched() {
        let strings = strings();
        assert_eq!(
            strings.get_plain_or("UIIT_MSG_LOGOUT_REMAIN_TIME", "fallback"),
            "It will take %d seconds to close the game."
        );
        assert_eq!(
            strings.get_plain_or("NO_SUCH_KEY", "verbatim <fallback"),
            "verbatim <fallback"
        );
    }

    /// The dialect is a closed tag set in shipped data, so anything malformed is
    /// passed through rather than swallowed.
    #[test]
    fn plain_text_passes_through_unterminated_markup() {
        assert_eq!(plain_text("unterminated <tag"), "unterminated <tag");
        assert_eq!(plain_text("a<br/>b<BR>c"), "a\nb\nc");
    }

    /// The two `UI / SND_BUTTON_CLICK` rows verbatim from the user's
    /// `media://server_dep/silkroad/textdata/effectsound.txt` (2026-08-16),
    /// preceded by the file's own legend line.
    const EFFECTSOUND_ROWS: &str = concat!(
        "//\tobject\thandle\tskill_ID\tevent1\tevent2\tevent3\tblank\tfolder\tfilename\tvolume\tdescription1\t\r\n",
        "\tUI\tSND_BUTTON_CLICK\t-\t-\t-\t-\t0\tui\\\tuibutton_a.wav\t80\tclick#1\t\r\n",
        "\tUI\tSND_BUTTON_CLICK\t-\t-\t-\t-\t0\tui\\\tuibutton_b.wav\t80\tclick#2\t\r\n",
    );

    /// The registry resource is what the UI click reads (#773): it must resolve
    /// the row's own asset path and volume, and answer `None` (never panic)
    /// while the table has not loaded yet.
    #[test]
    fn client_effect_sounds_resolves_the_ui_click_row() {
        let click = SoundAddress::new("UI", "SND_BUTTON_CLICK");

        let empty = ClientEffectSounds::default();
        assert!(empty.first(&click).is_none());
        assert!(empty.sounds(&click).is_empty());

        let loaded = ClientEffectSounds(Some(EffectSoundTable::parse(EFFECTSOUND_ROWS)));
        let row = loaded.first(&click).expect("UI/SND_BUTTON_CLICK");
        assert_eq!(row.asset_path(), "data://prim/snd/ui/uibutton_a.wav");
        assert_eq!(row.gain(), 0.8);
        assert_eq!(loaded.sounds(&click).len(), 2);
    }
}
