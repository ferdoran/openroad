//! In-game help book index (`gameguidedata.txt`) — the tree the Help window's
//! navigation drawer walks (#575).
//!
//! Idea: the file is a flat 7-column table that encodes a **two-level tree** in
//! two different ways at once. Column 3 says what a row is (`0` = category,
//! `1` = leaf) and column 4 means different things per kind: for a leaf it is
//! its parent's id, for a category it reads as a child count. The parent
//! column is the one that is unambiguous, so the tree is built **from the
//! leaves upward** and the category number is kept as a *stated* count that
//! callers can check against reality — it is exact for only 38 of the 44
//! categories in v1.188 (`docs/re/ui/game-guide-window.md` §4), and rounding
//! that into a rule would silently drop the six exceptions.
//!
//! Column 0 is an enable flag and **700 of the 1023 rows are disabled**: the
//! shipped guide is under a third of the authored one. Disabled rows are
//! parsed and kept, marked `enabled: false`, so enabling them is a data
//! decision rather than a code change.

use std::collections::HashMap;

/// What a row is, and the id it relates to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuideKind {
    /// A drawer heading. `stated_children` is column 4 read as a child count —
    /// see the module note: it is the data's claim, not a measurement.
    Category { stated_children: u32 },
    /// A page. `parent` is column 4, which for leaves is unambiguous.
    Leaf { parent: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideNode {
    /// Column 0: `1` ships in v1.188, `0` is authored-but-disabled.
    pub enabled: bool,
    /// Column 1, e.g. `1000`, `1001`, … `280000`.
    pub id: u32,
    /// Column 2: the Korean title as authored. Display goes through
    /// [`GuideNode::menu_key`], not through this.
    pub title: String,
    pub kind: GuideKind,
    /// Column 5: `SRO_GGW_*`, the body key resolving in `texthelp.txt`.
    /// `None` when the row ships no body (the start pages, some headings).
    pub body_key: Option<String>,
    /// Column 6: `SRO_GGW_MENU_*`, the drawer label key.
    pub menu_key: Option<String>,
}

/// The guide tree in file order (the drawer renders in this order).
#[derive(Debug, Clone, Default)]
pub struct GuideIndex {
    pub nodes: Vec<GuideNode>,
}

/// A category whose stated child count disagrees with the leaves that actually
/// name it as parent — the six known v1.188 exceptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildCountMismatch {
    pub category: u32,
    pub stated: u32,
    pub actual: u32,
}

/// The body/menu columns are empty on rows that carry no string key; anything
/// else is passed through verbatim rather than filtered against a guessed set
/// of placeholder spellings.
fn optional_key(field: &str) -> Option<String> {
    let field = field.trim();
    (!field.is_empty()).then(|| field.to_string())
}

impl GuideIndex {
    /// Parse the tab-separated file content (already decoded from UTF-16).
    /// Rows that are ragged, unparsable or carry an unknown depth are skipped
    /// rather than fatal: this is user-supplied PK2 data.
    pub fn parse(content: &str) -> Self {
        let mut nodes = Vec::new();
        for line in content.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 7 {
                continue;
            }
            let enabled = match fields[0].trim().trim_start_matches('\u{feff}') {
                "1" => true,
                "0" => false,
                _ => continue,
            };
            let Ok(id) = fields[1].trim().parse::<u32>() else {
                continue;
            };
            let Ok(column4) = fields[4].trim().parse::<u32>() else {
                continue;
            };
            let kind = match fields[3].trim() {
                "0" => GuideKind::Category {
                    stated_children: column4,
                },
                "1" => GuideKind::Leaf { parent: column4 },
                _ => continue,
            };
            nodes.push(GuideNode {
                enabled,
                id,
                title: fields[2].trim().to_string(),
                kind,
                body_key: optional_key(fields[5]),
                menu_key: optional_key(fields[6]),
            });
        }
        Self { nodes }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Categories in file order.
    pub fn categories(&self) -> impl Iterator<Item = &GuideNode> {
        self.nodes
            .iter()
            .filter(|n| matches!(n.kind, GuideKind::Category { .. }))
    }

    /// Leaves that name `category` as their parent, in file order.
    pub fn children_of(&self, category: u32) -> impl Iterator<Item = &GuideNode> {
        self.nodes
            .iter()
            .filter(move |n| n.kind == GuideKind::Leaf { parent: category })
    }

    /// Every category whose stated child count disagrees with the leaves that
    /// point at it. The caller decides what to do with them — the count is a
    /// checksum on the data, never the source of the tree.
    pub fn child_count_mismatches(&self) -> Vec<ChildCountMismatch> {
        let mut actual: HashMap<u32, u32> = HashMap::new();
        for node in &self.nodes {
            if let GuideKind::Leaf { parent } = node.kind {
                *actual.entry(parent).or_default() += 1;
            }
        }
        self.categories()
            .filter_map(|category| {
                let GuideKind::Category { stated_children } = category.kind else {
                    return None;
                };
                let measured = actual.get(&category.id).copied().unwrap_or(0);
                (measured != stated_children).then_some(ChildCountMismatch {
                    category: category.id,
                    stated: stated_children,
                    actual: measured,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Two categories, five leaves, one disabled row, one ragged line. The
    /// second category states 2 children but only one leaf points at it —
    /// the shape of the six v1.188 exceptions.
    const FIXTURE: &str = "1\t1000\t플레이가이드\t0\t3\t\tSRO_GGW_MENU_PLAY\r\n\
                           1\t1001\tCharacter\t1\t1000\tSRO_GGW_PG_GROWTH\tSRO_GGW_MENU_PG_GROWTH\r\n\
                           1\t1002\tInterface\t1\t1000\tSRO_GGW_PG_UI\tSRO_GGW_MENU_PG_UI\r\n\
                           0\t1003\tDisabled page\t1\t1000\tSRO_GGW_PG_OFF\tSRO_GGW_MENU_PG_OFF\r\n\
                           1\t7000\t연금술\t0\t2\t\tSRO_GGW_MENU_ALCHEMY\r\n\
                           1\t7001\tAlchemy\t1\t7000\tSRO_GGW_PG_ALCHEMY\tSRO_GGW_MENU_PG_ALCHEMY\r\n\
                           ragged\trow";

    #[test]
    fn parses_categories_and_leaves() {
        let index = GuideIndex::parse(FIXTURE);
        assert_eq!(index.nodes.len(), 6, "the ragged row is skipped, not fatal");
        assert_eq!(index.categories().count(), 2);
        assert_eq!(
            index.nodes[0].kind,
            GuideKind::Category { stated_children: 3 }
        );
        assert_eq!(index.nodes[1].kind, GuideKind::Leaf { parent: 1000 });
        assert_eq!(
            index.nodes[1].body_key.as_deref(),
            Some("SRO_GGW_PG_GROWTH")
        );
        // column 5 is empty on categories — no body, and no empty-string key
        assert_eq!(index.nodes[0].body_key, None);
        assert_eq!(
            index.nodes[0].menu_key.as_deref(),
            Some("SRO_GGW_MENU_PLAY")
        );
    }

    /// The 700 disabled rows are kept and marked, not dropped: enabling the
    /// rest of the authored guide must be a data decision.
    #[test]
    fn disabled_rows_are_kept_and_marked() {
        let index = GuideIndex::parse(FIXTURE);
        let disabled: Vec<&GuideNode> = index.nodes.iter().filter(|n| !n.enabled).collect();
        assert_eq!(disabled.len(), 1);
        assert_eq!(disabled[0].id, 1003);
        // and it is still a child of its category
        assert_eq!(index.children_of(1000).count(), 3);
    }

    /// The tree comes from the leaves' parent column; the category's own
    /// column-4 number is only a checksum, and in v1.188 it is wrong for six
    /// of the 44 categories.
    #[test]
    fn the_child_count_is_a_checksum_not_the_tree() {
        let index = GuideIndex::parse(FIXTURE);
        assert_eq!(index.children_of(7000).count(), 1);
        assert_eq!(
            index.child_count_mismatches(),
            vec![ChildCountMismatch {
                category: 7000,
                stated: 2,
                actual: 1,
            }],
            "1000 states 3 and has 3 rows, 7000 states 2 and has 1 — only the \
             real disagreement is reported"
        );
    }
}
