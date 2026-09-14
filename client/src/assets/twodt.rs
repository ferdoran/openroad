use std::convert::TryFrom;
use std::fmt::Debug;

use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
use bevy::color::Color;
use bevy::math::{Rect, Vec2};
use bevy::reflect::TypePath;
use bytes::{Buf, Bytes};
use num_enum::TryFromPrimitive;
use thiserror::Error;

use crate::assets::{Argb8888, Str128, Str256, Str64};
use crate::util::buf_ext::BufExt;

#[derive(Debug, TypePath, Asset)]
pub struct JMXV2DT {
    entry_count: u32,
    entries: Vec<Jmxv2dtEntry>,
}

pub type SroNewInterface = JMXV2DT;

/// Bytes per entry. `(filesize - 4) / entryCount == 976` holds exactly in all
/// 42 corpus files (2,312 entries), so a short tail means a truncated file
/// rather than a layout we do not know.
///
/// **Coordinate space, for whoever builds the first consumer**: these rects are
/// absolute coordinates in one flat design space, and a child is *not* clipped
/// to or re-based on its parent — 9 of the 42 files place at least one child at
/// an x or y smaller than their root's. Local layout is `child.xy - root.xy`,
/// which is a no-op for the roots authored at `(0,0)` and only those. See
/// `docs/formats/2dt-newinterface.md`.
const ENTRY_SIZE: usize = 976;

impl JMXV2DT {
    /// Every entry, in file order.
    pub fn entries(&self) -> &[Jmxv2dtEntry] {
        &self.entries
    }

    /// The `entry_count` the header declared — which may exceed
    /// `entries().len()` on a truncated file, and that difference is the
    /// signal a caller wants.
    pub fn declared_entry_count(&self) -> u32 {
        self.entry_count
    }

    /// The window's root. Exactly one entry per file carries `is_root = 1`
    /// across all 42 corpus files, so this is the descriptor's own idea of
    /// "the window", not a heuristic.
    pub fn root(&self) -> Option<&Jmxv2dtEntry> {
        self.entries.iter().find(|entry| entry.is_root())
    }

    /// Look an entry up by its `Id` — what `ContentId` points at on a tab
    /// button, and what `parent_id` refers to.
    pub fn entry(&self, id: u32) -> Option<&Jmxv2dtEntry> {
        self.entries.iter().find(|entry| entry.id() == id)
    }

    /// Look an entry up by its descriptor name (`CNIFBattleArenaRuleWnd`, …).
    pub fn entry_by_name(&self, name: &str) -> Option<&Jmxv2dtEntry> {
        self.entries.iter().find(|entry| entry.name() == name)
    }

    /// The direct children of `id`, in file order.
    pub fn children_of(&self, id: u32) -> impl Iterator<Item = &Jmxv2dtEntry> {
        self.entries
            .iter()
            .filter(move |entry| entry.parent_id() == id)
    }

    /// An entry's rect **relative to the window root**.
    ///
    /// 2DT rects are absolute coordinates in one flat design space: a child is
    /// neither clipped to nor offset from its parent, and 9 of the 42 corpus
    /// files place at least one child at an x or y *smaller* than their root's.
    /// So local layout is `child.xy - root.xy` — a no-op only for the roots
    /// authored at `(0,0)`, e.g. `arena_game_result.2dt`, which the client
    /// centres at runtime. Without a root the absolute rect is returned
    /// unchanged (#447).
    pub fn local_rect(&self, entry: &Jmxv2dtEntry) -> Rect {
        let rect = entry.rect();
        match self.root() {
            Some(root) => {
                let origin = root.rect().min;
                Rect::from_corners(rect.min - origin, rect.max - origin)
            }
            None => rect,
        }
    }

    fn from_bytes(bytes: &[u8]) -> JMXV2DT {
        let mut bytes = Bytes::copy_from_slice(bytes);
        // let entry_count:u32 = u32::from_ne_bytes(bytes[0..4].try_into().expect("i failed"));
        let entry_count = bytes.get_u32_le();
        let mut entries: Vec<Jmxv2dtEntry> = Vec::with_capacity(entry_count as usize);

        // Entries are fixed-size and read sequentially, so the only thing the
        // size arithmetic bought was a divide by `entry_count` — which is a
        // divide-by-zero on an empty file. Stop before it instead.
        let mut entries_data = Bytes::copy_from_slice(&bytes[0..]);
        for _ in 0..entry_count {
            if entries_data.remaining() < ENTRY_SIZE {
                break;
            }
            entries.push(Jmxv2dtEntry::from(&mut entries_data));
        }

        JMXV2DT {
            entry_count,
            entries,
        }
    }
}

#[derive(Debug)]
pub struct Jmxv2dtEntry {
    // 972 bits
    name: Str64,
    image: Str256,
    background: Str256,
    text: Str128,
    description: Str64,
    prototype: Str64,
    //  placeholder (lorem ipsum)
    /// Raw `NewInterfaceType`. Kept raw because the corpus is user data: an
    /// unknown value must not abort the load (it used to `expect`), and a
    /// caller that cares asks [`Jmxv2dtEntry::ni_type`].
    ni_type: u32,
    id: u32,
    parent_id: u32,
    grand_parent_id: u32,
    unk_02: u32,
    unk_03: u32,
    color: Argb8888,
    client_rectangle_x: u32,
    client_rectangle_y: u32,
    client_rectangle_width: u32,
    client_rectangle_height: u32,
    // UV Mapping
    top_left_x: f32,
    top_left_y: f32,
    top_right_x: f32,
    top_right_y: f32,
    bottom_right_x: f32,
    bottom_right_y: f32,
    bottom_left_x: f32,
    bottom_left_y: f32,
    unk_04: u32,
    // CommandID?
    /// `-1` in 7 corpus rows, so this is signed.
    content_id: i32,
    is_root: u32,
    unk_07: u32,
    unk_08: u32,
    unk_09: u32,
    unk_10: u32,
    unk_11: u32,
    unk_12: u32,
    unk_13: u32,
    unk_14: u32,
    unk_15: u32,
    unk_16: u32,
    unk_17: u32,
    unk_18: u32,
    unk_19: u32,
    style: Jmxv2dtStyle,
}

impl Jmxv2dtEntry {
    /// The descriptor's own name, e.g. `CNIFBattleArenaRuleWnd`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Foreground art path, relative to the PK2 root.
    pub fn image(&self) -> &str {
        &self.image
    }

    /// Background art path.
    pub fn background(&self) -> &str {
        &self.background
    }

    /// The `UI_STRING` key this entry displays, if any.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    /// The control type, `None` for a value no known enum member covers —
    /// this is user data, so an unknown type is a reason to skip one control,
    /// not to fail the file.
    pub fn ni_type(&self) -> Option<Jmxv2dtType> {
        Jmxv2dtType::try_from(self.ni_type).ok()
    }

    /// The raw type value, so an unknown one can still be reported.
    pub fn raw_ni_type(&self) -> u32 {
        self.ni_type
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn parent_id(&self) -> u32 {
        self.parent_id
    }

    pub fn grand_parent_id(&self) -> u32 {
        self.grand_parent_id
    }

    /// On a `CNIFTabButton` this is the `Id` of the pane the tab shows, so two
    /// tabs sharing a `ContentId` are two verbs over one pane layout. `-1` in
    /// 7 corpus rows, hence signed.
    pub fn content_id(&self) -> i32 {
        self.content_id
    }

    pub fn is_root(&self) -> bool {
        self.is_root != 0
    }

    pub fn style(&self) -> Jmxv2dtStyle {
        self.style
    }

    /// The entry's **absolute** rect in the descriptor's flat design space —
    /// see [`JMXV2DT::local_rect`] before positioning anything with it.
    pub fn rect(&self) -> Rect {
        Rect::new(
            self.client_rectangle_x as f32,
            self.client_rectangle_y as f32,
            (self.client_rectangle_x + self.client_rectangle_width) as f32,
            (self.client_rectangle_y + self.client_rectangle_height) as f32,
        )
    }

    /// The declared colour. The field is `ARGB_8888` despite the wiki calling
    /// it RGBA, matching the classic grammar's `COLOR` order.
    pub fn color(&self) -> Color {
        let [a, r, g, b] = self.color.to_be_bytes();
        Color::srgba_u8(r, g, b, a)
    }

    /// The eight UV floats, in the file's order: top-left, top-right,
    /// bottom-right, bottom-left.
    pub fn uvs(&self) -> [Vec2; 4] {
        [
            Vec2::new(self.top_left_x, self.top_left_y),
            Vec2::new(self.top_right_x, self.top_right_y),
            Vec2::new(self.bottom_right_x, self.bottom_right_y),
            Vec2::new(self.bottom_left_x, self.bottom_left_y),
        ]
    }
}

impl<T: Buf + BufExt> From<&mut T> for Jmxv2dtEntry {
    fn from(bytes: &mut T) -> Self {
        Self {
            name: bytes.get_fixed_size_string(64),
            image: bytes.get_fixed_size_string(256),
            background: bytes.get_fixed_size_string(256),
            text: bytes.get_fixed_size_string(128),
            description: bytes.get_fixed_size_string(64),
            prototype: bytes.get_fixed_size_string(64),
            ni_type: bytes.get_u32_le(),
            id: bytes.get_u32_le(),
            parent_id: bytes.get_u32_le(),
            grand_parent_id: bytes.get_u32_le(),
            unk_02: bytes.get_u32_le(),
            unk_03: bytes.get_u32_le(),
            color: bytes.get_u32_le(),
            client_rectangle_x: bytes.get_u32_le(),
            client_rectangle_y: bytes.get_u32_le(),
            client_rectangle_width: bytes.get_u32_le(),
            client_rectangle_height: bytes.get_u32_le(),
            top_left_x: bytes.get_f32_le(),
            top_left_y: bytes.get_f32_le(),
            top_right_x: bytes.get_f32_le(),
            top_right_y: bytes.get_f32_le(),
            bottom_right_x: bytes.get_f32_le(),
            bottom_right_y: bytes.get_f32_le(),
            bottom_left_x: bytes.get_f32_le(),
            bottom_left_y: bytes.get_f32_le(),
            unk_04: bytes.get_u32_le(),
            content_id: bytes.get_i32_le(),
            is_root: bytes.get_u32_le(),
            unk_07: bytes.get_u32_le(),
            unk_08: bytes.get_u32_le(),
            unk_09: bytes.get_u32_le(),
            unk_10: bytes.get_u32_le(),
            unk_11: bytes.get_u32_le(),
            unk_12: bytes.get_u32_le(),
            unk_13: bytes.get_u32_le(),
            unk_14: bytes.get_u32_le(),
            unk_15: bytes.get_u32_le(),
            unk_16: bytes.get_u32_le(),
            unk_17: bytes.get_u32_le(),
            unk_18: bytes.get_u32_le(),
            unk_19: bytes.get_u32_le(),
            style: Jmxv2dtStyle(bytes.get_i32_le()),
        }
    }
}

/// Text style, a `[Flags]` bitfield — **not** an enum.
///
/// Read as an enum over `{0, 256, 512, 65536}` it collapsed 2,092 of the
/// corpus' 2,312 entries to "none", because the dominant value is
/// `0x10100` (×2,070) = `LINE_CENTER | CENTER`, a combination no enum can
/// express. Full histogram: `0x10100` ×2070 · `0x10000` ×207 · `0x10200` ×21 ·
/// `0x200` ×6 · `0x0` ×6 · `0x100` ×1 · `0x20000` ×1.
///
/// Source: `SilkroadDoc-wiki/NewInterfaceStyle.md`, which carries `[Flags]`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Jmxv2dtStyle(pub i32);

impl Jmxv2dtStyle {
    pub const CENTER: i32 = 0x100;
    pub const RIGHT: i32 = 0x200;
    pub const LINE_CENTER: i32 = 0x10000;

    /// No style bits at all (6 corpus entries).
    pub fn is_none(&self) -> bool {
        self.0 == 0
    }

    /// Horizontally centred text.
    pub fn is_center(&self) -> bool {
        self.0 & Self::CENTER != 0
    }

    /// Right-aligned text.
    pub fn is_right(&self) -> bool {
        self.0 & Self::RIGHT != 0
    }

    /// Vertically centred within the line box.
    pub fn is_line_center(&self) -> bool {
        self.0 & Self::LINE_CENTER != 0
    }

    /// Bits outside the three known flags — `0x20000` appears on one corpus
    /// entry and has no documented meaning.
    pub fn unknown_bits(&self) -> i32 {
        self.0 & !(Self::CENTER | Self::RIGHT | Self::LINE_CENTER)
    }
}

#[derive(Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u32)]
pub enum Jmxv2dtType {
    CNIFMainFrame = 0,
    CNIFrame = 1,
    CNIFNormaltile = 2,
    CNIFStretch = 3,
    CNIFButton = 4,
    CNIFStatic = 5,
    CNIFEdit = 6,
    CNIFTextBox = 7,
    CNIFSlot = 8,
    CNIFLattice = 9,
    CNIFGauge = 10,
    CNIFCheckBox = 11,
    CNIFComboBox = 12,
    CNIFVirticalScroll = 13,
    CNIFPageManager = 14,
    CNIFBarWnd = 15,
    CNIFTabButton = 16,
    CNIFBothSidesGauge = 17,
    CNIFWnd = 18,
    CNIFSlideCtrl = 19,
    CNIFSpinButtonCtrl = 20,
}

#[derive(Default, bevy::reflect::TypePath)]
pub struct TwoDtLoader;

#[derive(Error, Debug)]
pub enum TwoDtLoaderError {}

impl AssetLoader for TwoDtLoader {
    type Asset = JMXV2DT;
    type Settings = ();
    type Error = TwoDtLoaderError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.unwrap();
        let bytes = &buf;
        let sni = SroNewInterface::from_bytes(bytes);
        Ok(sni)
    }

    fn extensions(&self) -> &[&str] {
        &["2dt"]
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// #294: `style` is a `[Flags]` bitfield. Read as an enum over
    /// `{0, 256, 512, 65536}` the dominant corpus value `0x10100` — which is
    /// simply `LINE_CENTER | CENTER` — matched nothing and collapsed to
    /// "none", taking 2,092 of the 2,312 entries with it.
    #[test]
    fn style_combines_flags_an_enum_could_not_express() {
        let style = Jmxv2dtStyle(0x10100);

        assert!(style.is_center());
        assert!(style.is_line_center());
        assert!(!style.is_right());
        assert!(!style.is_none());
        assert_eq!(style.unknown_bits(), 0);
    }

    /// The single values still read correctly, and 0 is genuinely "no style"
    /// (6 corpus entries).
    #[test]
    fn style_reads_the_single_flags_and_none() {
        assert!(Jmxv2dtStyle(Jmxv2dtStyle::RIGHT).is_right());
        assert!(!Jmxv2dtStyle(Jmxv2dtStyle::RIGHT).is_center());
        assert!(Jmxv2dtStyle(0).is_none());
    }

    /// One corpus entry carries `0x20000`, which no source documents — it must
    /// be surfaced rather than silently folded into a known flag.
    #[test]
    fn style_surfaces_undocumented_bits() {
        let style = Jmxv2dtStyle(0x20000);

        assert!(!style.is_center() && !style.is_right() && !style.is_line_center());
        assert_eq!(style.unknown_bits(), 0x20000);
    }

    /// #294: all six string fields were read latin1 + `unidecode`, turning the
    /// corpus' 2,206 valid-CP949 strings into transliterated garbage.
    #[test]
    fn string_fields_decode_cp949_instead_of_transliterating() {
        let mut field = vec![0xB9, 0xDA, 0xBD, 0xBA]; // CP949 "박스"
        field.resize(64, 0);
        let mut bytes = Bytes::from(field);

        assert_eq!(bytes.get_fixed_size_string(64), "박스");
    }

    /// `0xFD` is a legal CP949 lead byte *and* the stale exporter fill byte in
    /// 7 corpus files — where it always sits after the NUL terminator. The old
    /// helper stripped `0xFD` everywhere, so it ate the lead byte of real text
    /// in order to clean up padding it should never have reached. Terminating
    /// at the NUL handles both without special-casing the byte at all.
    #[test]
    fn fd_fill_after_the_terminator_is_dropped_but_a_lead_byte_is_kept() {
        let mut field = vec![0xFD, 0xC6, 0x00]; // CP949 char with an 0xFD lead, then NUL
        field.resize(16, 0xFD); // stale exporter fill past the terminator
        let mut bytes = Bytes::from(field);

        let decoded = bytes.get_fixed_size_string(16);

        assert_eq!(decoded.chars().count(), 1, "got {decoded:?}");
        assert!(
            !decoded.contains('\u{FFFD}'),
            "lead byte was eaten: {decoded:?}"
        );
    }

    /// A field shorter than its declared size must not slice out of bounds —
    /// the shared helper indexes `chunk()[..size]` unchecked.
    #[test]
    fn a_short_field_does_not_panic() {
        let mut bytes = Bytes::from_static(&[0x41, 0x42]);

        assert_eq!(bytes.get_fixed_size_string(64), "AB");
    }

    /// Byte-exact fixture of one 976-byte entry, so the accessors are tested
    /// against the real layout rather than against a constructor.
    fn entry_bytes(
        name: &str,
        ni_type: u32,
        id: u32,
        parent_id: u32,
        rect: (u32, u32, u32, u32),
        is_root: u32,
        content_id: i32,
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(ENTRY_SIZE);
        let mut string_field = |value: &str, size: usize| {
            let mut field = value.as_bytes().to_vec();
            field.resize(size, 0);
            out.extend_from_slice(&field);
        };
        string_field(name, 64); // name
        string_field("interface\\x.ddj", 256); // image
        string_field("", 256); // background
        string_field("UIIT_STT_X", 128); // text
        string_field("", 64); // description
        string_field("", 64); // prototype
        for value in [ni_type, id, parent_id, 0, 0, 0] {
            out.extend_from_slice(&value.to_le_bytes()); // type..unk_03
        }
        out.extend_from_slice(&0x80_11_22_33u32.to_le_bytes()); // color (ARGB)
        for value in [rect.0, rect.1, rect.2, rect.3] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for _ in 0..8 {
            out.extend_from_slice(&0f32.to_le_bytes()); // UVs
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // unk_04
        out.extend_from_slice(&content_id.to_le_bytes());
        out.extend_from_slice(&is_root.to_le_bytes());
        while out.len() < ENTRY_SIZE - 4 {
            out.extend_from_slice(&0u32.to_le_bytes()); // unk_07..unk_19
        }
        out.extend_from_slice(&0x10100i32.to_le_bytes()); // style
        assert_eq!(out.len(), ENTRY_SIZE);
        out
    }

    fn file_bytes(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut out = (entries.len() as u32).to_le_bytes().to_vec();
        for entry in entries {
            out.extend_from_slice(entry);
        }
        out
    }

    /// #540: the record was fully decoded and completely unreadable — every
    /// field private, no accessors, `#[allow(dead_code)]` on the struct. This
    /// is the end-to-end read the four 4th-gen window docs need.
    #[test]
    fn entries_are_readable_through_accessors() {
        let file = file_bytes(&[
            entry_bytes("CNIFRootWnd", 0, 10, 0, (413, 33, 372, 371), 1, -1),
            entry_bytes("CNIFTab", 16, 9, 10, (430, 60, 80, 24), 0, 28),
        ]);
        let parsed = JMXV2DT::from_bytes(&file);

        assert_eq!(parsed.declared_entry_count(), 2);
        assert_eq!(parsed.entries().len(), 2);

        let root = parsed.root().expect("exactly one entry carries is_root");
        assert_eq!(root.name(), "CNIFRootWnd");
        assert_eq!(root.id(), 10);
        assert_eq!(root.rect(), Rect::new(413.0, 33.0, 785.0, 404.0));
        assert_eq!(root.ni_type(), Some(Jmxv2dtType::CNIFMainFrame));
        assert_eq!(root.color(), Color::srgba_u8(0x11, 0x22, 0x33, 0x80));

        let tab = parsed.entry(9).expect("lookup by Id");
        assert!(!tab.is_root());
        assert_eq!(tab.parent_id(), 10);
        assert_eq!(tab.content_id(), 28); // the pane this tab shows
        assert_eq!(tab.text(), "UIIT_STT_X");
        assert_eq!(tab.image(), "interface\\x.ddj");
        assert!(tab.style().is_center());
        assert_eq!(parsed.entry_by_name("CNIFTab").map(|e| e.id()), Some(9));
        assert_eq!(parsed.children_of(10).count(), 1);
    }

    /// The coordinate rule that only bites windows *not* authored at (0,0):
    /// rects are absolute, so local layout is `child.xy - root.xy`, and a
    /// child may legitimately land at a negative local coordinate (9 of the 42
    /// corpus files do).
    #[test]
    fn local_rect_rebases_children_onto_the_root() {
        let file = file_bytes(&[
            entry_bytes("Root", 0, 1, 0, (413, 33, 372, 371), 1, -1),
            entry_bytes("Inside", 5, 2, 1, (430, 60, 80, 24), 0, 0),
            entry_bytes("Above", 5, 3, 1, (400, 20, 10, 10), 0, 0),
        ]);
        let parsed = JMXV2DT::from_bytes(&file);

        let inside = parsed.entry(2).unwrap();
        assert_eq!(parsed.local_rect(inside), Rect::new(17.0, 27.0, 97.0, 51.0));

        // authored above and left of its own root — absolute coords, no clip
        let above = parsed.entry(3).unwrap();
        assert_eq!(
            parsed.local_rect(above),
            Rect::new(-13.0, -13.0, -3.0, -3.0)
        );
    }

    /// `(filesize - 4) / entry_count == 976` holds exactly across all 42
    /// corpus files (2,312 entries), so a short tail is a truncated file and
    /// must yield the entries that are whole rather than panicking.
    #[test]
    fn entry_size_is_976_and_a_truncated_tail_is_dropped() {
        assert_eq!(ENTRY_SIZE, 976);
        let whole = entry_bytes("Root", 0, 1, 0, (0, 0, 10, 10), 1, -1);
        assert_eq!(4 + 2 * whole.len(), 4 + 2 * 976);

        let mut truncated = file_bytes(&[whole.clone(), whole]);
        truncated.truncate(4 + ENTRY_SIZE + 100); // second entry cut short
        let parsed = JMXV2DT::from_bytes(&truncated);
        assert_eq!(parsed.declared_entry_count(), 2);
        assert_eq!(parsed.entries().len(), 1, "only whole entries are kept");
    }

    /// An unknown `NewInterfaceType` used to `expect()` mid-parse and abort
    /// the load; it must degrade to "unknown control" instead.
    #[test]
    fn an_unknown_control_type_does_not_abort_the_file() {
        let file = file_bytes(&[entry_bytes("Weird", 0xDEAD, 1, 0, (0, 0, 1, 1), 1, -1)]);
        let parsed = JMXV2DT::from_bytes(&file);
        let entry = parsed.root().unwrap();
        assert_eq!(entry.ni_type(), None);
        assert_eq!(entry.raw_ni_type(), 0xDEAD);
    }
}
