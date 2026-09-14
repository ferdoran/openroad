use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::prelude::TypePath;
use std::collections::HashMap;
use thiserror::Error;

use crate::assets::textdata::actionwnddata::ActionWndData;
use crate::assets::textdata::characterdata::{CharacterData, CharacterDataRow};
use crate::assets::textdata::collectionbook::CollectionBookTable;
use crate::assets::textdata::dungeoninfo::DungeonInfo;
use crate::assets::textdata::effectsound::EffectSoundTable;
use crate::assets::textdata::gameguide::GuideIndex;
use crate::assets::textdata::itemdata::{ItemData, ItemDataRow};
use crate::assets::textdata::leveldata::LevelData;
use crate::assets::textdata::magicoption::{MagicOptionData, MagicOptionInfo};
use crate::assets::textdata::masterydata::MasteryData;
use crate::assets::textdata::names::TextdataNames;
use crate::assets::textdata::npcchat::NpcChat;
use crate::assets::textdata::questreward::{QuestRewardItems, QuestRewardModes};
use crate::assets::textdata::shops::ShopTable;
use crate::assets::textdata::skilldata::{SkillData, SkillDataRow};
use crate::assets::textdata::skilleffect::{parse_skilleffect, SkillEffectTable};
use crate::assets::textdata::skillgroup::SkillGroupTable;
use crate::assets::textdata::teleport::TeleportTable;
use crate::assets::textdata::uisystem::UiSystemText;
use crate::assets::textdata::worldmap::WorldMapTable;
use crate::assets::textdata::zonenames::ZoneNames;
use crate::assets::textdata::zonesound::ZoneSoundTable;

pub mod actionwnddata;
pub mod characterdata;
pub mod collectionbook;
pub mod decode;
pub mod dungeoninfo;
pub mod effectsound;
pub mod gameguide;
pub mod itemdata;
pub mod leveldata;
pub mod magicoption;
pub mod masterydata;
pub mod names;
pub mod npcchat;
pub mod questreward;
pub mod shops;
pub mod skilldata;
pub mod skilleffect;
pub mod skillgroup;
pub mod teleport;
pub mod uisystem;
pub mod worldmap;
pub mod zonenames;
pub mod zonesound;

#[derive(Default, bevy::reflect::TypePath)]
pub(crate) struct TextdataLoader;

/// Read + decode another textdata file from within a loader (registers it as
/// a load dependency). Used by the ref-shop chain, whose sibling files never
/// load standalone.
async fn read_sibling(
    load_context: &mut LoadContext<'_>,
    name: &str,
) -> Result<String, TextdataError> {
    let path = format!("media://server_dep/silkroad/textdata/{name}");
    let bytes = load_context
        .read_asset_bytes(path)
        .await
        .map_err(|e| TextdataError::UnknownTextdata(format!("shop chain file {name}: {e}")))?;
    Ok(decode::decode_textdata(&bytes))
}

#[derive(Asset, TypePath, Debug, Clone)]
pub enum Textdata {
    /// `actionwnddata.txt` — what the action window's 52 empty slots contain.
    ActionWnd(ActionWndData),
    CharacterData(CharacterData),
    /// `collectionbook_theme.txt` + its `collectionbook_item.txt` sibling.
    CollectionBook(CollectionBookTable),
    ItemData(ItemData),
    LevelData(LevelData),
    ZoneNames(ZoneNames),
    /// Quest reward mode table + its per-quest reward rows (two files).
    QuestRewards(QuestRewardModes, QuestRewardItems),
    Names(TextdataNames),
    SkillData(SkillData),
    SkillEffects(SkillEffectTable),
    MasteryData(MasteryData),
    SkillGroups(SkillGroupTable),
    MagicOption(MagicOptionData),
    UiSystem(UiSystemText),
    /// `gameguidedata.txt` — the in-game help book's category/page tree (#575).
    GameGuide(GuideIndex),
    /// `texthelp.txt` — the help bodies, keyed `SRO_GGW_*`. Same
    /// enabled/key/languages shape as textuisystem, so it shares that parser;
    /// the values are CIFPML markup, reduced for display by
    /// `plugins::textdata::plain_text`.
    GuideText(UiSystemText),
    /// NPC/quest speech strings (`textquest_speech&name.txt`) — same string
    /// table shape as textuisystem.
    SpeechText(UiSystemText),
    NpcChat(NpcChat),
    Shops(ShopTable),
    Teleport(TeleportTable),
    WorldMap(WorldMapTable),
    /// `Data.pk2:dungeon/dungeoninfo.txt` — dungeon id → `.dof` path (loads
    /// from the `data://` source, unlike the `media://` tables above).
    DungeonInfo(DungeonInfo),
    /// `regioninfo.txt` joined with its `effectenvsnd.txt` sibling: region id
    /// → zone name, BGM track and day/night ambient list (EP-22.1).
    ZoneSounds(ZoneSoundTable),
    /// `effectsound.txt` — the SFX registry: `(object, handle, skill_ID,
    /// event1..3)` → `.wav` path plus the row's own volume (EP-22.2).
    EffectSounds(EffectSoundTable),
}

#[derive(Error, Debug)]
pub enum TextdataError {
    #[error("unknown textdata file {0}")]
    UnknownTextdata(String),
    #[error("IO Error: {0}")]
    IO(std::io::Error),
}

impl AssetLoader for TextdataLoader {
    type Asset = Textdata;
    type Settings = ();
    type Error = TextdataError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let file_stem = load_context
            .path()
            .path()
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        if !file_stem.starts_with("actionwnddata")
            && !file_stem.starts_with("characterdata")
            && !file_stem.starts_with("collectionbook_theme")
            && !file_stem.starts_with("itemdata")
            && !file_stem.starts_with("leveldata")
            && !file_stem.starts_with("textzonename")
            && !file_stem.starts_with("textdata")
            && !file_stem.starts_with("skilldata")
            && !file_stem.starts_with("skilleffect")
            && !file_stem.starts_with("skillmasterydata")
            && !file_stem.starts_with("skillgroup")
            && !file_stem.starts_with("magicoption")
            && !file_stem.starts_with("gameguidedata")
            && !file_stem.starts_with("texthelp")
            && !file_stem.starts_with("textuisystem")
            && !file_stem.starts_with("textquest_speech")
            && !file_stem.starts_with("npcchat")
            && !file_stem.starts_with("refshopgroup")
            && !file_stem.starts_with("teleportdata")
            && !file_stem.starts_with("worldmap_mapinfo")
            && !file_stem.starts_with("dungeoninfo")
            && !file_stem.starts_with("refqusetreward")
            && !file_stem.starts_with("regioninfo")
            && !file_stem.starts_with("effectsound")
        {
            return Err(TextdataError::UnknownTextdata(format!(
                "{}",
                load_context.path().path().display()
            )));
        }

        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .await
            .map_err(|e| TextdataError::IO(e))?;
        // Shared decoder: BOM-sniff UTF-16 with CP949 fallback (EP-09.1).
        let content = decode::decode_textdata(&buf);

        if file_stem.starts_with("characterdata") {
            if file_stem.ends_with("characterdata") {
                // master file listing the actual data files
                let files = content
                    .lines()
                    .map(|l| l.to_lowercase())
                    .collect::<Vec<_>>();

                let mut data = HashMap::new();
                for file in files {
                    let char_data = load_context
                        .load_builder()
                        .load_untyped_value(format!(
                            "media://server_dep/silkroad/textdata/{}",
                            file
                        ))
                        .await
                        .unwrap();
                    let char_data = char_data.take::<Textdata>().unwrap();
                    if let Textdata::CharacterData(char_data) = char_data {
                        data.extend(char_data.0);
                    }
                }
                Ok(Textdata::CharacterData(CharacterData(data)))
            } else {
                let data = content
                    .lines()
                    .map(|l| {
                        l.split("\t")
                            .map(|s| String::from(s))
                            .collect::<Vec<String>>()
                    })
                    .filter(|l| l.len() > 10)
                    .map(|l| (l[1].parse::<i32>().unwrap(), CharacterDataRow(l)))
                    .collect::<HashMap<_, _>>();
                Ok(Textdata::CharacterData(CharacterData(data)))
            }
        } else if file_stem.starts_with("collectionbook_theme") {
            // the theme file is the entry point: its item sibling never loads
            // standalone, and the original's own loader reads both together.
            let items = read_sibling(load_context, "collectionbook_item.txt").await?;
            Ok(Textdata::CollectionBook(CollectionBookTable::parse(
                &content, &items,
            )))
        } else if file_stem.starts_with("actionwnddata") {
            // single file: the action window's command → icon/string bindings
            Ok(Textdata::ActionWnd(ActionWndData::parse(&content)))
        } else if file_stem.starts_with("refqusetreward") {
            // The mode table names no items and the item table names no mode,
            // so the pair is read together (like the ref-shop chain) — the
            // sibling never loads standalone. The original's filename typo
            // ("quset") is reproduced because that is what the archive holds.
            let items = read_sibling(load_context, "refquestrewarditems.txt").await?;
            Ok(Textdata::QuestRewards(
                QuestRewardModes::parse(&content),
                QuestRewardItems::parse(&items),
            ))
        } else if file_stem.starts_with("regioninfo") {
            // regioninfo is the entry point of the zone-sound join: it names
            // the zones and their sectors, effectenvsnd keys off the same
            // names and never loads standalone (EP-22.1).
            let sounds = read_sibling(load_context, "effectenvsnd.txt").await?;
            Ok(Textdata::ZoneSounds(ZoneSoundTable::parse(
                &content, &sounds,
            )))
        } else if file_stem.starts_with("effectsound") {
            // single file: the SFX registry (EP-22.2)
            Ok(Textdata::EffectSounds(EffectSoundTable::parse(&content)))
        } else if file_stem.starts_with("textzonename") {
            // single file, no master-list indirection
            Ok(Textdata::ZoneNames(ZoneNames::parse(&content)))
        } else if file_stem.starts_with("gameguidedata") {
            Ok(Textdata::GameGuide(GuideIndex::parse(&content)))
        } else if file_stem.starts_with("texthelp") {
            Ok(Textdata::GuideText(UiSystemText::parse(&content)))
        } else if file_stem.starts_with("textuisystem") {
            // single file: UIIT_* UI strings
            Ok(Textdata::UiSystem(UiSystemText::parse(&content)))
        } else if file_stem.starts_with("textquest_speech") {
            // NPC/quest speech strings (SN_*_BS greeting texts etc.)
            Ok(Textdata::SpeechText(UiSystemText::parse(&content)))
        } else if file_stem.starts_with("npcchat") {
            Ok(Textdata::NpcChat(NpcChat::parse(&content)))
        } else if file_stem.starts_with("refshopgroup") {
            // refshopgroup.txt is the entry point of the ref-shop chain: the
            // six sibling files are read raw here (they never load standalone)
            // and the whole chain is denormalized into one ShopTable.
            let mapping_group = read_sibling(load_context, "refmappingshopgroup.txt").await?;
            let mapping_tab = read_sibling(load_context, "refmappingshopwithtab.txt").await?;
            let shoptab = read_sibling(load_context, "refshoptab.txt").await?;
            let goods = read_sibling(load_context, "refshopgoods.txt").await?;
            let scrap = read_sibling(load_context, "refscrapofpackageitem.txt").await?;
            let prices = read_sibling(load_context, "refpricepolicyofitem.txt").await?;
            Ok(Textdata::Shops(ShopTable::assemble(
                &content,
                &mapping_group,
                &mapping_tab,
                &shoptab,
                &goods,
                &scrap,
                &prices,
            )))
        } else if file_stem.starts_with("teleportdata") {
            // teleportdata.txt is the entry point; the link + building tables
            // ride along.
            let links = read_sibling(load_context, "teleportlink.txt").await?;
            let buildings = read_sibling(load_context, "teleportbuilding.txt").await?;
            Ok(Textdata::Teleport(TeleportTable::parse(
                &content, &links, &buildings,
            )))
        } else if file_stem.starts_with("dungeoninfo") {
            Ok(Textdata::DungeonInfo(DungeonInfo::parse(&content)))
        } else if file_stem.starts_with("worldmap_mapinfo") {
            // worldmap_mapinfo.txt is the entry point; the POI table rides along.
            let localinfo = read_sibling(load_context, "worldmap_localinfo.txt").await?;
            Ok(Textdata::WorldMap(WorldMapTable::parse(
                &content, &localinfo,
            )))
        } else if file_stem.starts_with("textdataname") {
            // master file listing the name-table shards (textdata_object etc.)
            let files = content
                .lines()
                .map(|l| l.to_lowercase())
                .filter(|l| !l.trim().is_empty())
                .collect::<Vec<_>>();

            let mut data = HashMap::new();
            for file in files {
                let names = load_context
                    .load_builder()
                    .load_untyped_value(format!("media://server_dep/silkroad/textdata/{}", file))
                    .await
                    .unwrap();
                let names = names.take::<Textdata>().unwrap();
                if let Textdata::Names(names) = names {
                    data.extend(names.0);
                }
            }
            Ok(Textdata::Names(TextdataNames(data)))
        } else if file_stem.starts_with("textdata_") {
            Ok(Textdata::Names(TextdataNames::parse(&content)))
        } else if file_stem.starts_with("skilldata") {
            if file_stem.ends_with("skilldata") {
                // master file listing the actual data files
                let files = content
                    .lines()
                    .map(|l| l.to_lowercase())
                    .filter(|l| !l.trim().is_empty())
                    .collect::<Vec<_>>();

                let mut data = HashMap::new();
                for file in files {
                    let skill_data = load_context
                        .load_builder()
                        .load_untyped_value(format!(
                            "media://server_dep/silkroad/textdata/{}",
                            file
                        ))
                        .await
                        .unwrap();
                    let skill_data = skill_data.take::<Textdata>().unwrap();
                    if let Textdata::SkillData(skill_data) = skill_data {
                        data.extend(skill_data.0);
                    }
                }
                Ok(Textdata::SkillData(SkillData(data)))
            } else {
                let data = content
                    .lines()
                    .map(|l| {
                        l.split("\t")
                            .map(|s| String::from(s))
                            .collect::<Vec<String>>()
                    })
                    .filter(|l| l.len() > 100)
                    .filter_map(|l| l[1].parse::<i32>().ok().map(|id| (id, SkillDataRow(l))))
                    .collect::<HashMap<_, _>>();
                Ok(Textdata::SkillData(SkillData(data)))
            }
        } else if file_stem.starts_with("skilleffect") {
            // #section-structured, not a row table — dedicated parser
            Ok(Textdata::SkillEffects(parse_skilleffect(&content)))
        } else if file_stem.starts_with("skillmasterydata") {
            Ok(Textdata::MasteryData(MasteryData::parse(&content)))
        } else if file_stem.starts_with("skillgroup") {
            Ok(Textdata::SkillGroups(SkillGroupTable::parse(&content)))
        } else if file_stem.starts_with("leveldata") {
            // level \t exp-to-advance-from-this-level \t mastery-up SP cost
            // \t <job/pet columns ...> (first row is level 1; see LevelData
            // for the keying)
            let mut data = LevelData::default();
            for l in content
                .lines()
                .map(|l| l.split("\t").map(String::from).collect::<Vec<String>>())
                .filter(|l| l.len() > 2)
            {
                let Ok(level) = l[0].parse::<u8>() else {
                    continue;
                };
                if let Ok(exp) = l[1].parse::<u64>() {
                    data.exp.insert(level, exp);
                }
                if let Ok(sp) = l[2].parse::<u32>() {
                    data.mastery_sp.insert(level, sp);
                }
            }
            Ok(Textdata::LevelData(data))
        } else if file_stem.starts_with("magicoption") {
            // single file: id \t <codename cols>; col 1 = id, 2 = MATTR_* code,
            // 3 = display operator.
            let data = content
                .lines()
                .map(|l| l.split("\t").map(String::from).collect::<Vec<String>>())
                .filter(|l| l.len() > 4)
                .filter_map(|l| {
                    l[1].parse::<u32>().ok().map(|id| {
                        (
                            id,
                            MagicOptionInfo {
                                codename: l[2].clone(),
                                op: l[3].clone(),
                            },
                        )
                    })
                })
                .collect::<HashMap<_, _>>();
            Ok(Textdata::MagicOption(MagicOptionData(data)))
        } else {
            if file_stem.ends_with("itemdata") {
                // master file listing the actual data files
                let files = content
                    .lines()
                    .map(|l| l.to_lowercase())
                    .collect::<Vec<_>>();

                let mut data = HashMap::new();
                for file in files {
                    let item_data = load_context
                        .load_builder()
                        .load_untyped_value(format!(
                            "media://server_dep/silkroad/textdata/{}",
                            file
                        ))
                        .await
                        .unwrap();
                    let item_data = item_data.take::<Textdata>().unwrap();
                    if let Textdata::ItemData(item_data) = item_data {
                        data.extend(item_data.0);
                    }
                }
                Ok(Textdata::ItemData(ItemData(data)))
            } else {
                let data = content
                    .lines()
                    .map(|l| {
                        l.split("\t")
                            .map(|s| String::from(s))
                            .collect::<Vec<String>>()
                    })
                    .filter(|l| l.len() > 10)
                    .filter_map(|l| l[1].parse::<i32>().ok().map(|id| (id, ItemDataRow(l))))
                    .collect::<HashMap<_, _>>();
                Ok(Textdata::ItemData(ItemData(data)))
            }
        }
    }

    fn extensions(&self) -> &[&str] {
        &["txt"]
    }
}
