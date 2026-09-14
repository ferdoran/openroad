//! NPC shop inventories, denormalized from the vSRO ref-shop chain.
//!
//! Idea: the archive describes shops relationally across seven files —
//!   refshopgroup (group → NPC codename)
//!   → refmappingshopgroup (group → store)
//!   → refmappingshopwithtab (store → tab group)
//!   → refshoptab (tab group → tabs, with SN_/UIIT_ name keys)
//!   → refshopgoods (tab → slot → package codename)
//!   → refscrapofpackageitem (package → item codename + opt level)
//!   → refpricepolicyofitem (package → price, per currency)
//! — resolved once at load into `ShopTable`: NPC codename → tabs → slotted
//! goods. Join keys verified against the v1.188 data (e.g. NPC_CH_SMITH →
//! GROUP_STORE_CH_SMITH → STORE_CH_SMITH → STORE_CH_SMITH_GROUP1 →
//! STORE_CH_SMITH_TAB1 → PACKAGE_ITEM_CH_BLADE_01_A → ITEM_CH_BLADE_01_A /
//! 890 gold). The legacy shopdata/shoptabdata/shopitemdata files describe the
//! same shops in the old id-keyed form and are not used. Docs:
//! `docs/formats/shops.md`.

use std::collections::hash_map::Entry;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct ShopTable {
    pub by_npc: HashMap<String, ShopLayout>,
}

#[derive(Debug, Clone, Default)]
pub struct ShopLayout {
    /// One page per ref tab group — e.g. the Jangan armor shop's male and
    /// female pages, each with its own (≤4) tab strip. The vanilla page
    /// spinner flips through these.
    pub pages: Vec<ShopPage>,
}

impl ShopLayout {
    /// The wire tab index of a tab: its position in the store-wide flattened
    /// group→tab order (what the 0x7034 buy request's `tab` field means).
    pub fn wire_tab_index(&self, page: usize, tab: usize) -> u8 {
        let earlier: usize = self.pages[..page.min(self.pages.len())]
            .iter()
            .map(|group| group.tabs.len())
            .sum();
        (earlier + tab).min(u8::MAX as usize) as u8
    }
}

#[derive(Debug, Clone, Default)]
pub struct ShopPage {
    pub tabs: Vec<ShopTab>,
}

#[derive(Debug, Clone, Default)]
pub struct ShopTab {
    /// Tab label string id — `SN_*` (textdataname) or `UIIT_*` (textuisystem).
    pub name_key: String,
    /// Sorted by slot.
    pub goods: Vec<ShopGood>,
}

/// `refpricepolicyofitem` column 3: which currency the row's price is in.
///
/// The file never spells the currencies out — these readings come from the
/// tabs and NPCs each flag reaches (see `docs/formats/shops.md`). Flags with
/// no confident reading stay [`ShopCurrency::Other`] and render as a bare
/// amount: naming the wrong currency is worse than naming none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ShopCurrency {
    /// Flag 1 — every normal `STORE_*` tab across 66 real NPCs.
    #[default]
    Gold,
    /// Flag 2 — only ever on `MALL_*` tabs, which have no NPC.
    Silk,
    /// Flag 8 — only on `STORE_*_GUILD_TAB*` at `NPC_*_GUILD`.
    GuildPoints,
    /// Flag 32 — only on `STORE_{CH,KT,WC}_HONOR_TAB1/2` at the warehouses.
    HonorPoints,
    /// Flag 1024 — only on `STORE_BATTLE_ARENA_CH_TAB2` at
    /// `NPC_BATTLE_ARENA_EXCHANGER`.
    ArenaCoins,
    /// UNKNOWN: 64/128 (alternate payments for the same socket stones) and
    /// 256/512 (job goods at the two `*_CHANGER` NPCs, 512 only on `_RARE`).
    Other(u32),
}

impl ShopCurrency {
    fn from_flag(flag: u32) -> Self {
        match flag {
            1 => Self::Gold,
            2 => Self::Silk,
            8 => Self::GuildPoints,
            32 => Self::HonorPoints,
            1024 => Self::ArenaCoins,
            other => Self::Other(other),
        }
    }

    /// The unit shown next to a price, or `None` while the currency is still
    /// UNKNOWN — callers then render the bare number.
    pub fn label(&self) -> Option<&'static str> {
        match self {
            Self::Gold => Some("Gold"),
            Self::Silk => Some("Silk"),
            Self::GuildPoints => Some("Guild Points"),
            Self::HonorPoints => Some("Honor Points"),
            Self::ArenaCoins => Some("Arena Coins"),
            Self::Other(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShopGood {
    /// 0-based slot within the tab (6-wide grid rows).
    pub slot: u8,
    /// itemdata codename of the sold item (`ITEM_*`).
    pub item_codename: String,
    /// Enhancement level the store sells the item at (usually 0).
    pub opt_level: u8,
    /// Buy price (0 when no price policy row exists), in [`Self::currency`].
    pub price: u64,
    /// What [`Self::price`] is denominated in. Not always gold: 140 packages
    /// have no gold row at all.
    pub currency: ShopCurrency,
}

impl ShopTable {
    pub fn get(&self, npc_codename: &str) -> Option<&ShopLayout> {
        self.by_npc.get(npc_codename)
    }

    /// Assemble the denormalized table from the seven decoded file contents.
    pub fn assemble(
        shopgroup: &str,
        mapping_group: &str,
        mapping_tab: &str,
        shoptab: &str,
        goods: &str,
        scrap: &str,
        prices: &str,
    ) -> Self {
        // enabled rows only, split on tabs
        let rows = |content: &str| -> Vec<Vec<String>> {
            content
                .lines()
                .map(|l| {
                    l.split('\t')
                        .map(|f| f.trim().to_string())
                        .collect::<Vec<_>>()
                })
                .filter(|f| f.len() > 2 && f[0].trim_start_matches('\u{feff}') == "1")
                .collect()
        };

        // group codename -> npc codename (refshopgroup: svc|country|id|group|npc)
        let mut npc_of_group: HashMap<String, String> = HashMap::new();
        for row in rows(shopgroup) {
            if row.len() > 4 && !row[4].is_empty() && row[4] != "xxx" {
                npc_of_group.insert(row[3].clone(), row[4].clone());
            }
        }
        // group -> stores (refmappingshopgroup: svc|country|group|store)
        let mut stores_of_group: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows(mapping_group) {
            if row.len() > 3 {
                stores_of_group
                    .entry(row[2].clone())
                    .or_default()
                    .push(row[3].clone());
            }
        }
        // store -> tab groups (refmappingshopwithtab: svc|country|store|tabgroup)
        let mut tabgroups_of_store: HashMap<String, Vec<String>> = HashMap::new();
        for row in rows(mapping_tab) {
            if row.len() > 3 {
                tabgroups_of_store
                    .entry(row[2].clone())
                    .or_default()
                    .push(row[3].clone());
            }
        }
        // tab group -> ordered tabs (refshoptab: svc|country|id|tab|tabgroup|name)
        let mut tabs_of_group: HashMap<String, Vec<(u32, String, String)>> = HashMap::new();
        for row in rows(shoptab) {
            if row.len() > 5 {
                let id = row[2].parse().unwrap_or(u32::MAX);
                tabs_of_group.entry(row[4].clone()).or_default().push((
                    id,
                    row[3].clone(),
                    row[5].clone(),
                ));
            }
        }
        for tabs in tabs_of_group.values_mut() {
            tabs.sort_by_key(|(id, _, _)| *id);
        }
        // tab -> goods (refshopgoods: svc|country|tab|package|slot)
        let mut goods_of_tab: HashMap<String, Vec<(u8, String)>> = HashMap::new();
        for row in rows(goods) {
            if row.len() > 4 {
                let slot = row[4].parse().unwrap_or(0);
                goods_of_tab
                    .entry(row[2].clone())
                    .or_default()
                    .push((slot, row[3].clone()));
            }
        }
        // package -> (item codename, opt level)
        // (refscrapofpackageitem: svc|country|package|item|opt|...)
        let mut item_of_package: HashMap<String, (String, u8)> = HashMap::new();
        for row in rows(scrap) {
            if row.len() > 4 {
                let opt = row[4].parse().unwrap_or(0);
                item_of_package.insert(row[2].clone(), (row[3].clone(), opt));
            }
        }
        // package -> price (refpricepolicyofitem:
        // svc|country|package|currency flag|0|price)
        //
        // A package can carry one row per currency it is buyable in — 237 of
        // 933 do. Gold wins where it is offered, which keeps NPC stores exactly
        // as they were; the rest keep their own currency instead of being
        // mislabelled as gold (140 packages have no gold row at all).
        let mut price_of_package: HashMap<String, (u64, ShopCurrency)> = HashMap::new();
        for row in rows(prices) {
            if row.len() > 5 {
                let Ok(price) = row[5].parse() else { continue };
                let currency = ShopCurrency::from_flag(row[3].parse().unwrap_or_default());
                match price_of_package.entry(row[2].clone()) {
                    Entry::Vacant(slot) => {
                        slot.insert((price, currency));
                    }
                    Entry::Occupied(mut slot) => {
                        if currency == ShopCurrency::Gold {
                            slot.insert((price, currency));
                        }
                    }
                }
            }
        }

        // group id order for determinism, and MERGE multi-group NPCs: one NPC
        // is mapped by two shop groups (NPC_WC_WAREHOUSE_W's two honor shops)
        // — a plain insert would keep an arbitrary one per run (HashMap
        // iteration order). NPC_TD_THIEF_SELL has a single group and was
        // previously conflated with NPC_TD_THIEF_BUY.
        let mut groups: Vec<(&String, &String)> = npc_of_group.iter().collect();
        groups.sort_by(|a, b| a.0.cmp(b.0));
        let mut by_npc: HashMap<String, ShopLayout> = HashMap::new();
        for (group, npc) in groups {
            let mut layout = ShopLayout::default();
            for store in stores_of_group.get(group).into_iter().flatten() {
                for tabgroup in tabgroups_of_store.get(store).into_iter().flatten() {
                    let mut page = ShopPage::default();
                    for (_, tab_codename, name_key) in
                        tabs_of_group.get(tabgroup).into_iter().flatten()
                    {
                        let mut tab = ShopTab {
                            name_key: name_key.clone(),
                            goods: Vec::new(),
                        };
                        for (slot, package) in goods_of_tab.get(tab_codename).into_iter().flatten()
                        {
                            let Some((item_codename, opt_level)) = item_of_package.get(package)
                            else {
                                continue;
                            };
                            let (price, currency) =
                                price_of_package.get(package).copied().unwrap_or_default();
                            tab.goods.push(ShopGood {
                                slot: *slot,
                                item_codename: item_codename.clone(),
                                opt_level: *opt_level,
                                price,
                                currency,
                            });
                        }
                        tab.goods.sort_by_key(|good| good.slot);
                        page.tabs.push(tab);
                    }
                    if !page.tabs.is_empty() {
                        layout.pages.push(page);
                    }
                }
            }
            if !layout.pages.is_empty() {
                by_npc
                    .entry(npc.clone())
                    .or_default()
                    .pages
                    .append(&mut layout.pages);
            }
        }
        ShopTable { by_npc }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn assembles_the_smith_chain() {
        let shopgroup = "1\t15\t978\tGROUP_STORE_CH_SMITH\tNPC_CH_SMITH\t-1\txxx\n\
                         1\t15\t977\tGROUP_MALL\txxx\t-1\txxx\n";
        let mapping_group = "1\t15\tGROUP_STORE_CH_SMITH\tSTORE_CH_SMITH\n";
        let mapping_tab = "1\t15\tSTORE_CH_SMITH\tSTORE_CH_SMITH_GROUP1\n";
        let shoptab = "1\t15\t2403\tSTORE_CH_SMITH_TAB2\tSTORE_CH_SMITH_GROUP1\tSN_TAB_SHIELD\n\
                       1\t15\t2402\tSTORE_CH_SMITH_TAB1\tSTORE_CH_SMITH_GROUP1\tSN_TAB_WEAPON\n";
        let goods = "1\t15\tSTORE_CH_SMITH_TAB1\tPACKAGE_ITEM_CH_BLADE_01_A\t3\n\
                     1\t15\tSTORE_CH_SMITH_TAB1\tPACKAGE_ITEM_CH_SWORD_01_A\t0\n";
        let scrap = "1\t15\tPACKAGE_ITEM_CH_SWORD_01_A\tITEM_CH_SWORD_01_A\t0\t0\t62\n\
                     1\t15\tPACKAGE_ITEM_CH_BLADE_01_A\tITEM_CH_BLADE_01_A\t0\t0\t69\n";
        let prices = "1\t15\tPACKAGE_ITEM_CH_SWORD_01_A\t1\t0\t150\n\
                      1\t15\tPACKAGE_ITEM_CH_BLADE_01_A\t1\t0\t890\n";

        let table = ShopTable::assemble(
            shopgroup,
            mapping_group,
            mapping_tab,
            shoptab,
            goods,
            scrap,
            prices,
        );
        let smith = table.get("NPC_CH_SMITH").unwrap();
        assert_eq!(smith.pages.len(), 1);
        let tabs = &smith.pages[0].tabs;
        assert_eq!(tabs.len(), 2);
        // tabs ordered by their refshoptab row id
        assert_eq!(tabs[0].name_key, "SN_TAB_WEAPON");
        // goods ordered by slot
        let goods = &tabs[0].goods;
        assert_eq!(goods[0].slot, 0);
        assert_eq!(goods[0].item_codename, "ITEM_CH_SWORD_01_A");
        assert_eq!(goods[0].price, 150);
        assert_eq!(goods[1].slot, 3);
        assert_eq!(goods[1].item_codename, "ITEM_CH_BLADE_01_A");
        assert_eq!(goods[1].price, 890);
        // groups without an NPC produce no shop
        assert_eq!(table.by_npc.len(), 1);
    }

    #[test]
    fn keeps_tab_groups_as_pages() {
        // armor-shop shape: one store, two tab groups (male/female)
        let shopgroup = "1\t15\t979\tGROUP_STORE_CH_ARMOR\tNPC_CH_ARMOR\t-1\txxx\n";
        let mapping_group = "1\t15\tGROUP_STORE_CH_ARMOR\tSTORE_CH_ARMOR\n";
        let mapping_tab = "1\t15\tSTORE_CH_ARMOR\tSTORE_CH_ARMOR_GROUP1\n\
                           1\t15\tSTORE_CH_ARMOR\tSTORE_CH_ARMOR_GROUP2\n";
        let shoptab = "1\t15\t2405\tSTORE_CH_ARMOR_TAB1\tSTORE_CH_ARMOR_GROUP1\tSN_TAB_HEAVYARMOR\n\
                       1\t15\t2406\tSTORE_CH_ARMOR_TAB2\tSTORE_CH_ARMOR_GROUP1\tSN_TAB_LIGHTARMOR\n\
                       1\t15\t2408\tSTORE_CH_ARMOR_TAB4\tSTORE_CH_ARMOR_GROUP2\tSN_TAB_HEAVYARMOR\n";
        let goods = "1\t15\tSTORE_CH_ARMOR_TAB4\tPACKAGE_ITEM_CH_W_HEAVY_01_BA_A\t0\n";
        let scrap = "1\t15\tPACKAGE_ITEM_CH_W_HEAVY_01_BA_A\tITEM_CH_W_HEAVY_01_BA_A\t0\t0\t40\n";
        let prices = "1\t15\tPACKAGE_ITEM_CH_W_HEAVY_01_BA_A\t1\t0\t250\n";

        let table = ShopTable::assemble(
            shopgroup,
            mapping_group,
            mapping_tab,
            shoptab,
            goods,
            scrap,
            prices,
        );
        let armor = table.get("NPC_CH_ARMOR").unwrap();
        assert_eq!(armor.pages.len(), 2);
        assert_eq!(armor.pages[0].tabs.len(), 2);
        assert_eq!(armor.pages[1].tabs.len(), 1);
        assert_eq!(
            armor.pages[1].tabs[0].goods[0].item_codename,
            "ITEM_CH_W_HEAVY_01_BA_A"
        );
        // the wire tab index counts across all groups: page-2 tab 0 = tab 2
        assert_eq!(armor.wire_tab_index(1, 0), 2);
        assert_eq!(armor.wire_tab_index(0, 1), 1);
    }

    #[test]
    fn merges_multi_group_npcs_deterministically() {
        // real shape: NPC_WC_WAREHOUSE_W is mapped by GROUP_STORE_WC_HONOR
        // AND GROUP_STORE_WC_HONOR2 — both shops must survive, ordered by
        // group codename (a plain insert dropped one at random)
        let shopgroup = "1\t15\t990\tGROUP_STORE_WC_HONOR2\tNPC_WC_WAREHOUSE_W\t-1\txxx\n\
                         1\t15\t989\tGROUP_STORE_WC_HONOR\tNPC_WC_WAREHOUSE_W\t-1\txxx\n";
        let mapping_group = "1\t15\tGROUP_STORE_WC_HONOR\tSTORE_A\n\
                             1\t15\tGROUP_STORE_WC_HONOR2\tSTORE_B\n";
        let mapping_tab = "1\t15\tSTORE_A\tGROUP_A1\n\
                           1\t15\tSTORE_B\tGROUP_B1\n";
        let shoptab = "1\t15\t2500\tTAB_A\tGROUP_A1\tSN_TAB_HONOR\n\
                       1\t15\t2501\tTAB_B\tGROUP_B1\tSN_TAB_HONOR2\n";
        let goods = "1\t15\tTAB_A\tPACKAGE_A\t0\n\
                     1\t15\tTAB_B\tPACKAGE_B\t0\n";
        let scrap = "1\t15\tPACKAGE_A\tITEM_A\t0\t0\t1\n\
                     1\t15\tPACKAGE_B\tITEM_B\t0\t0\t1\n";
        let prices = "1\t15\tPACKAGE_A\t1\t0\t10\n\
                      1\t15\tPACKAGE_B\t1\t0\t20\n";

        let table = ShopTable::assemble(
            shopgroup,
            mapping_group,
            mapping_tab,
            shoptab,
            goods,
            scrap,
            prices,
        );
        let shop = table.get("NPC_WC_WAREHOUSE_W").unwrap();
        assert_eq!(shop.pages.len(), 2);
        // sorted by group codename: HONOR before HONOR2
        assert_eq!(shop.pages[0].tabs[0].goods[0].item_codename, "ITEM_A");
        assert_eq!(shop.pages[1].tabs[0].goods[0].item_codename, "ITEM_B");
    }

    /// Build a one-NPC, one-tab store around a single package's price rows.
    fn assemble_one_good(prices: &str) -> ShopGood {
        let table = ShopTable::assemble(
            "1\t15\t1\tGROUP_X\tNPC_X\t-1\txxx\n",
            "1\t15\tGROUP_X\tSTORE_X\n",
            "1\t15\tSTORE_X\tGROUP_X1\n",
            "1\t15\t1\tTAB_X\tGROUP_X1\tSN_TAB_X\n",
            "1\t15\tTAB_X\tPACKAGE_X\t0\n",
            "1\t15\tPACKAGE_X\tITEM_X\t0\t0\t1\n",
            prices,
        );
        table.get("NPC_X").unwrap().pages[0].tabs[0].goods[0].clone()
    }

    /// refpricepolicyofitem col 3 is a currency bitflag, and a package carries
    /// one row per currency it sells in. 140 packages have no gold row at all;
    /// those used to render their silk price as gold.
    #[test]
    fn price_currency_comes_from_the_policy_flag() {
        let gold = assemble_one_good("1\t15\tPACKAGE_X\t1\t0\t890\n");
        assert_eq!((gold.price, gold.currency), (890, ShopCurrency::Gold));

        // a mall-only package: flag 2, no gold row anywhere
        let silk = assemble_one_good("1\t15\tPACKAGE_X\t2\t0\t25\n");
        assert_eq!((silk.price, silk.currency), (25, ShopCurrency::Silk));

        let honor = assemble_one_good("1\t15\tPACKAGE_X\t32\t0\t1000\n");
        assert_eq!(honor.currency, ShopCurrency::HonorPoints);
    }

    /// Gold wins wherever it is offered, whichever order the rows appear in —
    /// so NPC stores keep the prices they had under first-row-wins.
    #[test]
    fn gold_wins_over_other_currencies_in_either_order() {
        // the shipped order: gold first (true for all 793 gold packages)
        let gold_first =
            assemble_one_good("1\t15\tPACKAGE_X\t1\t0\t2700000\n1\t15\tPACKAGE_X\t32\t0\t1000\n");
        assert_eq!(
            (gold_first.price, gold_first.currency),
            (2700000, ShopCurrency::Gold)
        );

        // and the reverse, which the old first-row-wins would have mispriced
        let gold_last =
            assemble_one_good("1\t15\tPACKAGE_X\t32\t0\t1000\n1\t15\tPACKAGE_X\t1\t0\t2700000\n");
        assert_eq!(
            (gold_last.price, gold_last.currency),
            (2700000, ShopCurrency::Gold)
        );
    }

    /// Flags with no confident reading must not be labelled — 64/128/256/512
    /// are still UNKNOWN, so they render as a bare amount.
    #[test]
    fn unmapped_currency_flags_stay_unlabelled() {
        let good = assemble_one_good("1\t15\tPACKAGE_X\t256\t0\t26\n");
        assert_eq!(good.currency, ShopCurrency::Other(256));
        assert_eq!(good.currency.label(), None);
        for (flag, expected) in [
            (1, Some("Gold")),
            (2, Some("Silk")),
            (8, Some("Guild Points")),
            (32, Some("Honor Points")),
            (1024, Some("Arena Coins")),
            (64, None),
            (128, None),
            (512, None),
        ] {
            assert_eq!(
                ShopCurrency::from_flag(flag).label(),
                expected,
                "flag {flag}"
            );
        }
    }

    /// A package with no price row at all stays free-but-gold rather than
    /// silently inheriting another package's currency.
    #[test]
    fn missing_price_row_defaults_to_zero_gold() {
        let good = assemble_one_good("1\t15\tPACKAGE_OTHER\t2\t0\t99\n");
        assert_eq!((good.price, good.currency), (0, ShopCurrency::Gold));
    }
}
