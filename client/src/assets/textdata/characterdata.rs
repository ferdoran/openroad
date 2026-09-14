use bevy::asset::Asset;
use bevy::prelude::TypePath;
use packets::agent::pet::CosKind;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::ops::{Deref, Index};

// TODO: There are other files that use tab-separated rows as well.
//  Only thing that's different are number and order of fields.
//  Should also be made immutably readable so that we can prevent cloning / copying of data.
#[derive(Debug, Clone)]
pub struct CharacterDataRow(pub(crate) Vec<String>);

#[derive(Asset, TypePath, Debug, Clone)]
pub struct CharacterData(pub HashMap<i32, CharacterDataRow>);

/// Ceiling on a knockdown's prone dwell, in seconds.
///
/// Not a gameplay value — a **sanity clamp** on an `[U]` column. If
/// `KO_RecoverTime` turns out to be ticks or tenths rather than milliseconds,
/// the raw number could pin a body to the floor for minutes; this bounds the
/// damage from that misreading to something a player would call "a long
/// knockdown" rather than "the game froze". Chosen as the longest dwell that
/// still reads as one knockdown rather than a bug.
pub const KNOCKDOWN_RECOVER_MAX_SECS: f32 = 5.0;

#[repr(usize)]
#[allow(dead_code)]
enum ChardataFields {
    ID = 1,
    CodeName,
    // RefObjCommon OrgObjCodeOfObjName128: the code name of the base object a
    // variant reuses the model of. `xxx` for base objects; a code name (e.g.
    // `MOB_CH_WATERGHOST`) for summon clones/variants that carry no own model.
    OrgObjCode = 4,
    // RefObjCommon NameStrID: the `SN_*` key resolved against textdataname to a
    // localized display name (shared column with itemdata, see `itemdata.rs`).
    NameStrId = 5,
    // RefObjCommon DescStrID128. On growth-pet ladder rows this holds the *next*
    // stage's code name (`COS_P_WOLF_001` → `COS_P_WOLF_002`, `xxx` at the top) —
    // the chain the 0x30C9 ModelChanged update walks. See
    // docs/re/gamedata/textdata-characterdata.md §3.
    DescStrId128 = 6,
    // TypeID1..4 (RefObjCommon schema, shared with itemdata): the character
    // family and its subtype. TID1 == 1 is the bionic/character family.
    TypeId1 = 9,
    TypeId2,
    TypeId3,
    TypeId4,
    // RefObjCommon: ... DecayTime(13), Country(14), Rarity(15)
    Rarity = 15,
    // RefObjChar Speed1/Speed2: walk/run speed in world units per second
    // (players are 16/50; COS_C mounts run 90-150, COS_T transports 36-50).
    // Column numbers per docs/re/gamedata/textdata-characterdata.md §3
    // (SR_Db2Media/Settings.cs:43-51).
    Speed1 = 46,
    Speed2 = 47,
    ResourcePath = 52,
    // RefObjCommon AssocFileIcon_128 — the object's own 32x32 UI icon, the
    // third of the `52-56 AssocFile{Obj,Drop,Icon,1,2}_128` run
    // (docs/re/gamedata/textdata-characterdata.md §3, SR_Db2Media/Settings.cs).
    // Corpus-checked against the user's Media.pk2: 5,579 of the 5,587 COS rows
    // (TID 1/2/3/*) carry one, e.g. ref 6106 `COS_P_WOLF_001` ->
    // `cos\cos_p_wolf_01.ddj`. This is the only per-COS icon source there is —
    // the summon *item*'s icon does not reach the laddered growth stages,
    // which have no 1:1 item.
    AssocFileIcon = 54,
    // RefObjChar tail (after the shared RefObjCommon columns): Lvl(57),
    // CharGender(58), MaxHP(59). Verified against Media.pk2 characterdata:
    // MOB_CH_MANGNYANG lvl 1 / 54 HP, MOB_CH_TIGERWOMAN lvl 20 / 598720 HP,
    // NPCs 0 / 0. CharGender: CHAR_CH_MAN_* = 1, CHAR_CH_WOMAN_* = 0, 2 on
    // gender-neutral monsters — matches itemdata's Sex encoding.
    Level = 57,
    Gender = 58,
    MaxHp = 59,
    // RefObjChar: InventorySize(61) — the COS cargo-grid capacity for
    // transports (COS_T_DONKEY 32 ... COS_T_COW3 135; 0 for ride-only mounts);
    // CanBeVehicle(66)/CanControl(67) — 1 on rideable/commandable COS.
    InventorySize = 61,
    CanBeVehicle = 66,
    CanControl = 67,
    // RefObjChar Knockdown / KO_RecoverTime. These map 1:1 onto go-sro
    // `model/ref_char.go:13-36` (`… ExpToGive, Knockdown, KORecoveryTime, …`),
    // per docs/re/gamedata/textdata-characterdata.md §3.
    //
    // Read for the knockdown animation's prone dwell: the wire says a hit
    // knocked its target down (the displacement arms of 0xB070/0xB071) but
    // nowhere says for how long, and the original's timing is code-side. This
    // is the only *data* answer to that question, and it is per-victim — which
    // is the right granularity, since lying on the ground is a property of the
    // body that fell, not of the skill that felled it.
    //
    // The **unit is `[U]`**: the doc calls both columns "knockdown flags" and
    // no census of their value distribution exists. `ClientCharacterData::
    // knockdown_recover_secs` therefore refuses implausible values rather than
    // trusting the column blindly — see there.
    Knockdown = 81,
    KoRecoverTime = 82,
}

impl Index<ChardataFields> for Vec<String> {
    type Output = String;

    fn index(&self, index: ChardataFields) -> &Self::Output {
        &self[index as usize]
    }
}

impl Display for CharacterDataRow {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut s = String::new();
        self.0.iter().enumerate().for_each(|(i, e)| {
            s.push_str(&*format!("{} -> '{}'", i, e));
        });
        f.write_str(&s)
    }
}

impl Deref for CharacterData {
    type Target = HashMap<i32, CharacterDataRow>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl CharacterDataRow {
    /// The `.bsr` model path, or `None` when the row has no own model — the
    /// column is `xxx`/blank for summon clones and invisible helpers (e.g.
    /// `MOB_CH_WATERGHOST_CLON`, the "water ghost slave"). Loading `xxx`
    /// produced spurious `Path not found: res/xxx` errors, so callers skip
    /// the model when this is `None` (the entity still spawns, indexed).
    pub fn resource_path(&self) -> Option<String> {
        let path = &self.0[ChardataFields::ResourcePath];
        if path.is_empty() || path == "xxx" {
            return None;
        }
        Some(format!("data://res/{}", path.replace("\\", "/")))
    }

    pub fn code_name(&self) -> &String {
        &self.0[ChardataFields::CodeName]
    }

    /// The object's own UI icon as an asset path (`media://icon/...`), or
    /// `None` when the column is blank or a placeholder.
    ///
    /// Same shape as [`ItemDataRow::icon_path`](crate::assets::textdata::itemdata::ItemDataRow::icon_path)
    /// — both columns hold a backslashed, mixed-case path under `icon/`, and
    /// `bevy_pk2` is case-insensitive, so lowercasing is only for consistency.
    pub fn icon_path(&self) -> Option<String> {
        let path = self.0.get(ChardataFields::AssocFileIcon as usize)?.trim();
        if path.is_empty() || path == "xxx" {
            return None;
        }
        Some(format!(
            "media://icon/{}",
            path.replace('\\', "/").to_lowercase()
        ))
    }

    /// Code name of the base object this row reuses the model of (a summon
    /// clone / variant), or `None` for a base object (`xxx`/blank).
    pub fn org_obj_code(&self) -> Option<&str> {
        let code = &self.0[ChardataFields::OrgObjCode];
        (!code.is_empty() && code != "xxx").then_some(code.as_str())
    }

    /// The `SN_*` name key (RefObjCommon NameStrID), or `None` when the column is
    /// blank. Resolve it to a localized display name via `ClientTextNames::name`,
    /// mirroring how the inventory tooltip names items.
    pub fn name_key(&self) -> Option<&str> {
        let key = &self.0[ChardataFields::NameStrId];
        (!key.is_empty()).then_some(key.as_str())
    }

    /// The character's SRO type tuple `(TypeID1..4)`, or `None` if the columns
    /// are unparseable. `TID1 == 1` is the character family; `TID2` splits it
    /// into player (1) vs NPC/monster (2).
    pub fn type_ids(&self) -> Option<(u32, u32, u32, u32)> {
        let field = |f: ChardataFields| self.0[f].parse::<u32>().ok();
        Some((
            field(ChardataFields::TypeId1)?,
            field(ChardataFields::TypeId2)?,
            field(ChardataFields::TypeId3)?,
            field(ChardataFields::TypeId4)?,
        ))
    }

    /// A player character (`TID1/2 == 1/1`): uses the full player spawn record
    /// (inventory/avatar lists, name, guild, ...).
    pub fn is_player(&self) -> bool {
        matches!(self.type_ids(), Some((1, 1, _, _)))
    }

    /// Monster rarity class: 0 normal, 1 champion, 3 unique, 4 giant, ...
    /// (verified against Media.pk2 characterdata: MOB_CH_TIGERWOMAN = 3).
    pub fn rarity(&self) -> u8 {
        self.0[ChardataFields::Rarity].parse().unwrap_or(0)
    }

    /// The character's level (RefObjChar `Lvl`), or `None` when the column is
    /// missing/zero — NPCs carry 0 there, so "no level" and "NPC" coincide.
    pub fn level(&self) -> Option<u32> {
        let level = self.0.get(ChardataFields::Level as usize)?.parse().ok()?;
        (level > 0).then_some(level)
    }

    /// The character's maximum HP (RefObjChar `MaxHP`), or `None` when the
    /// column is missing/zero (NPCs).
    pub fn max_hp(&self) -> Option<u32> {
        let hp = self.0.get(ChardataFields::MaxHp as usize)?.parse().ok()?;
        (hp > 0).then_some(hp)
    }

    /// How long this body stays prone after a knockdown (RefObjChar
    /// `KO_RecoverTime`, col 82), in seconds — or `None` when the column is
    /// missing, zero, or reads as something other than a duration.
    ///
    /// **The column's unit is `[U]`.** It maps 1:1 onto go-sro's
    /// `KORecoveryTime`, but `docs/re/gamedata/textdata-characterdata.md` calls
    /// it a "knockdown flag" and no census of its values exists, so this cannot
    /// simply be trusted as milliseconds. Two guards make a wrong guess
    /// harmless rather than absurd:
    ///
    /// - values of 0 or 1 are read as a **flag**, not a duration ("this body
    ///   can be knocked down"), and yield `None`;
    /// - the result is clamped to [`KNOCKDOWN_RECOVER_MAX_SECS`], so a column
    ///   that turns out to be ticks or tenths cannot pin a body to the floor.
    ///
    /// `None` sends the caller to the animation's own authored length instead
    /// (`player::play_knockdowns`), which is data-derived too. Either way no
    /// number is invented here.
    pub fn knockdown_recover_secs(&self) -> Option<f32> {
        let raw: u32 = self
            .0
            .get(ChardataFields::KoRecoverTime as usize)?
            .trim()
            .parse()
            .ok()?;
        // 0/1 is the flag reading, not a 1 ms recovery.
        if raw <= 1 {
            return None;
        }
        Some((raw as f32 / 1000.0).min(KNOCKDOWN_RECOVER_MAX_SECS))
    }

    /// The player character's sex (RefObjChar `CharGender`): `"Woman"` (0) or
    /// `"Man"` (1); `None` for gender-neutral rows (2 — monsters/NPCs). Encoded
    /// like itemdata's Sex column, so it compares directly against
    /// `ItemDataRow::gender()` for equip checks.
    pub fn gender(&self) -> Option<&'static str> {
        match self.0.get(ChardataFields::Gender as usize)?.trim() {
            "0" => Some("Woman"),
            "1" => Some("Man"),
            _ => None,
        }
    }

    /// A monster (`TID1/2/3 == 1/2/1`): NPC record plus a trailing rarity byte.
    /// Non-player, non-monster, non-COS character-family rows (plain NPCs) use
    /// the plain NPC record.
    pub fn is_monster(&self) -> bool {
        matches!(self.type_ids(), Some((1, 2, 1, _)))
    }

    /// A COS (callable object summon): `TID1/2/3 == 1/2/3`, subtype in TID4.
    /// The tid3 gate is load-bearing — `1/2/4/*` holds `COS_GUARD_*` fortress
    /// guards *and* `STRUCTURE_*`/`MOB_FW_*` rows, so tid4 alone is ambiguous
    /// (corpus census 2026-08-14, docs/re/systems/pet-growth-cos.md).
    pub fn cos_kind(&self) -> Option<CosKind> {
        match self.type_ids() {
            Some((1, 2, 3, tid4)) => CosKind::from_type_id4(tid4 as u8),
            _ => None,
        }
    }

    /// RefObjChar `Speed1` — walk speed in world units/s, `None` when missing.
    pub fn walk_speed(&self) -> Option<f32> {
        let speed: f32 = self.0.get(ChardataFields::Speed1 as usize)?.parse().ok()?;
        (speed > 0.0).then_some(speed)
    }

    /// RefObjChar `Speed2` — run speed in world units/s, `None` when missing.
    pub fn run_speed(&self) -> Option<f32> {
        let speed: f32 = self.0.get(ChardataFields::Speed2 as usize)?.parse().ok()?;
        (speed > 0.0).then_some(speed)
    }

    /// RefObjChar `InventorySize` — transport cargo capacity, `None` when 0.
    pub fn inventory_size(&self) -> Option<u8> {
        let size: u8 = self
            .0
            .get(ChardataFields::InventorySize as usize)?
            .parse()
            .ok()?;
        (size > 0).then_some(size)
    }

    /// RefObjChar `CanBeVehicle` — whether players can board this character.
    pub fn can_be_vehicle(&self) -> bool {
        self.0
            .get(ChardataFields::CanBeVehicle as usize)
            .is_some_and(|v| v.trim() == "1")
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn typed_row(tid: (u32, u32, u32, u32)) -> CharacterDataRow {
        let mut fields = vec![String::new(); 53];
        fields[9] = tid.0.to_string();
        fields[10] = tid.1.to_string();
        fields[11] = tid.2.to_string();
        fields[12] = tid.3.to_string();
        CharacterDataRow(fields)
    }

    fn recover_row(raw: &str) -> CharacterDataRow {
        let mut fields = vec![String::new(); 90];
        fields[ChardataFields::KoRecoverTime as usize] = raw.to_string();
        CharacterDataRow(fields)
    }

    /// The plain reading: milliseconds into seconds.
    #[test]
    fn ko_recover_time_reads_as_milliseconds() {
        assert_eq!(recover_row("1500").knockdown_recover_secs(), Some(1.5));
    }

    /// 0 and 1 are the *flag* reading the docs describe ("knockdown flags"),
    /// not a 1 ms recovery. Treating them as durations would make every such
    /// body snap upright the instant it landed.
    #[test]
    fn a_flag_valued_column_is_not_a_duration() {
        assert_eq!(recover_row("0").knockdown_recover_secs(), None);
        assert_eq!(recover_row("1").knockdown_recover_secs(), None);
        // ...and so is a missing or unparseable column
        assert_eq!(recover_row("").knockdown_recover_secs(), None);
        assert_eq!(CharacterDataRow(vec![]).knockdown_recover_secs(), None);
    }

    /// The column's unit is unverified, so an absurd value is clamped rather
    /// than trusted — a body pinned to the floor for minutes would read as a
    /// freeze, not a knockdown.
    #[test]
    fn an_implausible_recovery_is_clamped() {
        assert_eq!(
            recover_row("600000").knockdown_recover_secs(),
            Some(KNOCKDOWN_RECOVER_MAX_SECS)
        );
    }

    #[test]
    fn classifies_character_subtypes() {
        let player = typed_row((1, 1, 1, 1));
        assert!(player.is_player() && !player.is_monster());

        let monster = typed_row((1, 2, 1, 0));
        assert!(monster.is_monster() && !monster.is_player());

        // Plain NPCs are neither player nor monster (the resolver's NPC fallback).
        let npc = typed_row((1, 2, 2, 0));
        assert!(!npc.is_player() && !npc.is_monster());
    }

    #[test]
    fn cos_kind_requires_the_tid3_gate() {
        assert_eq!(typed_row((1, 2, 3, 1)).cos_kind(), Some(CosKind::Vehicle));
        assert_eq!(typed_row((1, 2, 3, 2)).cos_kind(), Some(CosKind::Transport));
        assert_eq!(typed_row((1, 2, 3, 3)).cos_kind(), Some(CosKind::GrowthPet));
        assert_eq!(typed_row((1, 2, 3, 4)).cos_kind(), Some(CosKind::GrabPet));
        assert_eq!(typed_row((1, 2, 3, 5)).cos_kind(), Some(CosKind::Fellow));

        // 1/2/4/* is COS_GUARD_*/STRUCTURE_* territory — tid4 alone would
        // misclassify these as Vehicle/PickPet.
        assert_eq!(typed_row((1, 2, 4, 1)).cos_kind(), None);
        assert_eq!(typed_row((1, 2, 4, 4)).cos_kind(), None);
        // Unknown tid4 and non-character families classify as nothing.
        assert_eq!(typed_row((1, 2, 3, 6)).cos_kind(), None);
        assert_eq!(typed_row((3, 3, 3, 2)).cos_kind(), None);
        // COS rows are neither player nor monster.
        let cos = typed_row((1, 2, 3, 2));
        assert!(!cos.is_player() && !cos.is_monster());
    }

    #[test]
    fn speeds_and_inventory_read_their_columns() {
        // COS_T_HORSE1-shaped row: walk 24 / run 48 / cargo 33 / vehicle 1.
        let mut fields = vec![String::new(); 68];
        fields[46] = "24".to_string();
        fields[47] = "48".to_string();
        fields[61] = "33".to_string();
        fields[66] = "1".to_string();
        let row = CharacterDataRow(fields);
        assert_eq!(row.walk_speed(), Some(24.0));
        assert_eq!(row.run_speed(), Some(48.0));
        assert_eq!(row.inventory_size(), Some(33));
        assert!(row.can_be_vehicle());

        // Zero/short rows read as absent, not as 0-speed.
        let short = CharacterDataRow(vec![String::new(); 53]);
        assert_eq!(short.walk_speed(), None);
        assert_eq!(short.run_speed(), None);
        assert_eq!(short.inventory_size(), None);
        assert!(!short.can_be_vehicle());
    }

    #[test]
    fn unparseable_types_classify_as_nothing() {
        let blank = CharacterDataRow(vec![String::new(); 53]);
        assert!(!blank.is_player() && !blank.is_monster());
        assert_eq!(blank.type_ids(), None);
    }

    #[test]
    fn level_and_max_hp_read_refobjchar_tail() {
        let mut fields = vec![String::new(); 61];
        fields[57] = "20".to_string();
        fields[59] = "598720".to_string();
        let row = CharacterDataRow(fields);
        assert_eq!(row.level(), Some(20));
        assert_eq!(row.max_hp(), Some(598720));

        // NPC rows carry 0/0 there; short rows (test fixtures) have no tail.
        let mut npc = vec![String::new(); 61];
        npc[57] = "0".to_string();
        npc[59] = "0".to_string();
        let npc = CharacterDataRow(npc);
        assert_eq!(npc.level(), None);
        assert_eq!(npc.max_hp(), None);
        let short = CharacterDataRow(vec![String::new(); 53]);
        assert_eq!(short.level(), None);
        assert_eq!(short.max_hp(), None);
    }

    #[test]
    fn name_key_reads_col_5_and_skips_blanks() {
        let mut fields = vec![String::new(); 53];
        fields[5] = "SN_MOB_THIEF_NPC".to_string();
        let row = CharacterDataRow(fields);
        assert_eq!(row.name_key(), Some("SN_MOB_THIEF_NPC"));

        let blank = CharacterDataRow(vec![String::new(); 53]);
        assert_eq!(blank.name_key(), None);
    }
}
