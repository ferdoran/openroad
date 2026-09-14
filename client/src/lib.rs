// Parser-only library surface of the client crate.
//
// The client binary keeps its full module tree in main.rs; this lib target
// exposes only the pure SRO format parsers (bsr/bms/bmt/bsk/ban/ddj and
// their helpers) so external tools (tools/src/bin/bsr2glb) can reuse them
// without duplicating byte-level format knowledge. The subset is closed:
// these modules reference only each other, never plugins/scenes/commands.
//
// Trade-off: the listed files compile twice (once per target) and their
// #[cfg(test)] suites run for both, which is benign. Extend the list if a
// new transitive module reference appears.
pub mod util {
    pub mod binread;
    pub mod buf_ext;
    pub mod mesh;
    pub mod mips;
    /// Tween helpers. Only here because `assets::intro_scene` builds its
    /// camera `Sequence` from them; like the rest of this surface the module
    /// references nothing outside `util`.
    pub mod tweening_ext;
}

pub mod assets {
    pub mod ainav;
    pub mod ban;
    pub mod bms;
    pub mod bmt;
    pub mod bsk;
    pub mod bsr;
    pub mod ddj;
    pub mod dof;
    /// The `.intro` cutscene camera path. Needed by
    /// `tools/src/bin/intro_convert`, which converts the original's
    /// `script/intro/<name>.txt` camera block into one (#569) — the parser and
    /// the converter must not hold two copies of the field layout.
    pub mod intro_scene;
    pub mod nvm;
    /// Textdata tables. Verified closed like the rest of this surface: every
    /// `use crate::` in `assets/textdata/**` points at `assets` only, never at
    /// plugins/scenes/commands. Needed by `tools/src/bin/pk2_probe` to
    /// histogram itemdata/skilldata columns through the real parsers (and by
    /// `dungeon_scan` for the dungeoninfo table + string decoder).
    pub mod textdata;
    pub mod tile_tint;
}
