//! `effectsound.txt` — the client's sound-effect registry (EP-22.2).
//!
//! The idea: a sound in this table is addressed by a *tuple*, not by an id.
//! Line 1 of the file is its own column legend —
//! `// object handle skill_ID event1 event2 event3 blank folder filename volume description1` —
//! so `(object, handle, skill_ID, event1..3)` names the situation
//! ("PLAYER swings a SWORD", "UI clicks a button") and the row answers with a
//! `folder`+`filename` under `Data.pk2:prim/snd/` plus a per-row `volume`
//! (0-100). That volume is the number a call site cannot invent, which is why
//! the table has to be loaded rather than hard-coding paths where the sound is
//! played. The file is CP949 with no BOM (`decode.rs`).
//!
//! Measured in the user's own `Media.pk2` (2026-08-16), 6,583 lines:
//!
//! * 5,674 data rows over 292 distinct `object` values, registering 5,083
//!   sounding addresses — 390 of them carry more than one row
//!   (`PCF_KANGSI/VOC_AVOID` has 7), so an address maps to a *list* of rows,
//!   not to one row. The original picks among the variants; we keep them in
//!   file order and let the caller choose.
//! * `-` is the wildcard/none marker and appears in every column: 2,154 rows
//!   have no `skill_ID`, 27 have no `filename` at all (a mute row), and 17
//!   have no `volume`. It must never become the literal key `"-"`.
//! * 1,861 distinct `.wav` paths resolve under `prim/snd/`; 38 of them are not
//!   in the archive. A missing file is data, not a bug — the row is kept and
//!   the asset server simply fails that load.
//! * The `blank` column is 0/1/2/…/23, never used here; `description1` is the
//!   Korean editor comment. Neither is parsed.

use std::collections::HashMap;

/// Where `folder`+`filename` are rooted: the sound tree of `Data.pk2`. The
/// existing hard-coded call site (`data://prim/snd/ui/uibutton_a.wav`) proves
/// the prefix, and all 1,861 distinct paths resolve under it.
pub const SOUND_ROOT: &str = "prim/snd/";

/// The address of a sound: the six key columns, with `-` mapped to `None`.
///
/// Stored owned because the table owns its keys; a lookup builds one of these
/// per call, which is fine at the rate sounds are triggered.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct SoundAddress {
    pub object: String,
    pub handle: String,
    pub skill_id: Option<String>,
    pub events: [Option<String>; 3],
}

impl SoundAddress {
    /// `(object, handle)` with no skill and no events — the shape of the UI
    /// and item rows.
    pub fn new(object: &str, handle: &str) -> Self {
        Self {
            object: norm(object),
            handle: norm(handle),
            skill_id: None,
            events: [None, None, None],
        }
    }

    /// Narrow the address to one skill (`skill_ID` column).
    pub fn with_skill(mut self, skill_id: &str) -> Self {
        self.skill_id = wildcard(skill_id).map(norm);
        self
    }

    /// Narrow the address by the `event1..3` columns, in that order.
    pub fn with_events(mut self, events: [Option<&str>; 3]) -> Self {
        self.events = events.map(|e| e.and_then(wildcard).map(norm));
        self
    }
}

/// One row: which file to play and how loud the table says it should be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectSound {
    /// Archive-relative path, e.g. `prim/snd/ui/uibutton_a.wav`. Lowercased,
    /// forward slashes: `bevy_pk2` normalizes lookups the same way
    /// (`bevy_pk2/src/pk2/archive.rs:70`), and the table mixes `Player\` with
    /// `player\`.
    pub path: String,
    /// The row's own `volume` column, 0-100. `None` where the column is `-`.
    pub volume: Option<u8>,
}

impl EffectSound {
    /// Row volume as a linear gain factor to scale the FX channel with. A row
    /// without a volume plays at the channel's own level (1.0) rather than
    /// silently — `-` means "not stated", not "zero".
    pub fn gain(&self) -> f32 {
        self.volume.unwrap_or(100) as f32 / 100.0
    }

    /// Full asset path for the `data://` source the sound tree lives in.
    pub fn asset_path(&self) -> String {
        format!("data://{}", self.path)
    }
}

/// The parsed registry: address -> the rows that answer it, in file order.
#[derive(Debug, Clone, Default)]
pub struct EffectSoundTable {
    rows: HashMap<SoundAddress, Vec<EffectSound>>,
    row_count: usize,
    mute_rows: usize,
}

impl EffectSoundTable {
    /// Parse the decoded file. Comment lines (`//` in the first column) and
    /// short rows are skipped; a row whose `filename` is `-` is counted as a
    /// mute row and produces no entry.
    pub fn parse(content: &str) -> Self {
        let mut table = EffectSoundTable::default();
        for line in content.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            // legend/section comments carry `//` in the marker column
            if fields.len() < 11 || fields[0].trim().starts_with("//") {
                continue;
            }
            let object = fields[1].trim();
            let handle = fields[2].trim();
            if object.is_empty() || handle.is_empty() {
                continue;
            }
            table.row_count += 1;

            let Some(file) = wildcard(fields[9].trim()) else {
                table.mute_rows += 1;
                continue;
            };
            let folder = wildcard(fields[8].trim()).unwrap_or("");
            let sound = EffectSound {
                path: sound_path(folder, file),
                volume: wildcard(fields[10].trim()).and_then(|v| v.parse::<u8>().ok()),
            };
            let address = SoundAddress::new(object, handle)
                .with_skill(fields[3].trim())
                .with_events([
                    Some(fields[4].trim()),
                    Some(fields[5].trim()),
                    Some(fields[6].trim()),
                ]);
            table.rows.entry(address).or_default().push(sound);
        }
        table
    }

    /// Every row registered at an address, in file order (empty when unknown).
    pub fn sounds(&self, address: &SoundAddress) -> &[EffectSound] {
        self.rows.get(address).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The first row at an address — what a call site with no reason to pick a
    /// variant should play.
    pub fn first(&self, address: &SoundAddress) -> Option<&EffectSound> {
        self.sounds(address).first()
    }

    /// Distinct addresses in the table.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    #[allow(dead_code)] // clippy's len()/is_empty() pair; no consumer yet
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Data rows read, including the mute ones.
    pub fn row_count(&self) -> usize {
        self.row_count
    }

    /// Rows whose `filename` is `-`: an address that deliberately plays
    /// nothing.
    pub fn mute_rows(&self) -> usize {
        self.mute_rows
    }
}

/// `-` (and the empty string) is the table's wildcard/none marker.
fn wildcard(field: &str) -> Option<&str> {
    match field {
        "" | "-" => None,
        other => Some(other),
    }
}

/// Key columns are compared case-insensitively: the table writes `PLAYER` but
/// its folders show it is not consistent about case.
fn norm(field: &str) -> String {
    field.to_ascii_uppercase()
}

/// `folder` + `filename` under [`SOUND_ROOT`], normalized to lowercase
/// forward-slash form. Folder cells are usually `ui\` but some omit the
/// trailing separator (`Event\Summer_ghost`), so it is added when missing.
fn sound_path(folder: &str, file: &str) -> String {
    let mut path = String::from(SOUND_ROOT);
    if !folder.is_empty() {
        path.push_str(&folder.replace('\\', "/"));
        if !path.ends_with('/') {
            path.push('/');
        }
    }
    path.push_str(&file.replace('\\', "/"));
    path.to_ascii_lowercase()
}

#[cfg(test)]
mod test {
    use super::*;

    /// Real rows from the user's `Media.pk2` copy of `effectsound.txt`
    /// (2026-08-16), tabs and all: the legend line, a section comment, the two
    /// UI click variants, an item row, a skill-keyed row, a mute row and a row
    /// with no volume.
    const SAMPLE: &str = "//\tobject\thandle\tskill_ID\tevent1\tevent2\tevent3\tblank\tfolder\tfilename\tvolume\tdescription1\t\n\
//\tUI\t\t\t\t\t\t\t\t\t\t\t\n\
\tITEM\tSND_EQUIP\t-\tSWORD\t-\t-\t0\tui\\\titSword.wav\t80\tequip\t\n\
\tUI\tSND_BUTTON_CLICK\t-\t-\t-\t-\t0\tui\\\tuibutton_a.wav\t80\tclick#1\t\n\
\tUI\tSND_BUTTON_CLICK\t-\t-\t-\t-\t0\tui\\\tuibutton_b.wav\t80\tclick#2\t\n\
\tCOS_P_JINN\tSND_DMG\tPSKILL_P_JINN_03_ATTACK01\t-\t-\t-\t0\tCOS\\\tjinn_attack01.wav\t100\thit\t\n\
\tCOS_P_KANGAROO1\tVOC_DEATH\t-\t-\t-\t-\t0\tCOS\\\t-\t100\tsilent\t\n\
\tPLAYER\tSND_WALK1\t-\tFIELD\tDIRT\t-\t0\tEvent\\Summer_ghost\tstep.wav\t-\tstep\t\n";

    fn table() -> EffectSoundTable {
        EffectSoundTable::parse(SAMPLE)
    }

    #[test]
    fn parses_effectsound_rows_and_skips_comments() {
        let table = table();
        // 6 data rows (the legend and the section comment are not rows), of
        // which one is mute. The 5 sounding rows share one address (the two
        // click variants), so 4 addresses are registered.
        assert_eq!(table.row_count(), 6);
        assert_eq!(table.mute_rows(), 1);
        assert_eq!(table.len(), 4);
    }

    #[test]
    fn resolves_ui_click_path_and_row_volume() {
        let table = table();
        let click = table
            .first(&SoundAddress::new("UI", "SND_BUTTON_CLICK"))
            .expect("UI/SND_BUTTON_CLICK row");
        assert_eq!(click.path, "prim/snd/ui/uibutton_a.wav");
        assert_eq!(click.asset_path(), "data://prim/snd/ui/uibutton_a.wav");
        assert_eq!(click.volume, Some(80));
        assert_eq!(click.gain(), 0.8);
    }

    #[test]
    fn keeps_all_variants_of_one_address_in_file_order() {
        let table = table();
        let variants = table.sounds(&SoundAddress::new("UI", "SND_BUTTON_CLICK"));
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[1].path, "prim/snd/ui/uibutton_b.wav");
    }

    #[test]
    fn wildcards_are_none_not_the_literal_dash() {
        let table = table();
        // The ITEM row's event1 is SWORD; its skill_ID and event2/3 are `-`.
        let equip = SoundAddress::new("ITEM", "SND_EQUIP").with_events([
            Some("SWORD"),
            Some("-"),
            Some("-"),
        ]);
        assert_eq!(equip.skill_id, None);
        assert_eq!(equip.events, [Some("SWORD".to_string()), None, None]);
        assert!(table.first(&equip).is_some());
        // and the same address written with an explicit literal `-` skill does
        // not become a different key
        assert_eq!(equip, equip.clone().with_skill("-"));
    }

    #[test]
    fn skill_keyed_rows_need_their_skill_id() {
        let table = table();
        let addr = SoundAddress::new("COS_P_JINN", "SND_DMG");
        assert!(table.first(&addr).is_none());
        let with_skill = addr.with_skill("pskill_p_jinn_03_attack01");
        assert_eq!(
            table.first(&with_skill).map(|s| s.path.as_str()),
            Some("prim/snd/cos/jinn_attack01.wav")
        );
    }

    #[test]
    fn mute_row_registers_no_sound() {
        let table = table();
        assert!(table
            .first(&SoundAddress::new("COS_P_KANGAROO1", "VOC_DEATH"))
            .is_none());
    }

    #[test]
    fn folder_without_separator_and_missing_volume() {
        let table = table();
        let step = table
            .first(&SoundAddress::new("PLAYER", "SND_WALK1").with_events([
                Some("FIELD"),
                Some("DIRT"),
                None,
            ]))
            .expect("PLAYER/SND_WALK1 row");
        assert_eq!(step.path, "prim/snd/event/summer_ghost/step.wav");
        // `-` volume is "not stated": the row plays at the channel's level.
        assert_eq!(step.volume, None);
        assert_eq!(step.gain(), 1.0);
    }
}
