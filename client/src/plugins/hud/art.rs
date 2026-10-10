//! Descriptor art paths, in one place.
//!
//! IDEA. A `.2dt` descriptor names its art the way the original client stores
//! it: a CP949 Windows path. Our asset ids are lowercase, forward-slashed and
//! rooted at the `media://` source. Three windows each wrote that conversion
//! out by hand and each one assumed the descriptor path is *relative to*
//! `interface`, so each prefixed `media://interface/`.
//!
//! Measured against the corpus, that assumption is wrong for almost every real
//! path: of 957 art strings in `res_ui/*.2dt`, **955 already start with
//! `interface\`** — only two (in `nifeventword.2dt`) start somewhere else
//! (`icon\item\etc\...`). The old conversion therefore produced
//! `media://interface/interface/...`, which no reader resolves. It stayed
//! invisible because the fallback asset reader answers a missing texture with a
//! placeholder image, and because all three copies' tests fed hand-written
//! prefix-less strings that the data does not contain.
//!
//! So the rule is: make the path lowercase and forward-slashed, then root it at
//! `media://` — adding `interface/` only when the descriptor did not.

/// The asset source every descriptor's art lives in.
const MEDIA_ROOT: &str = "media://";
/// The folder the two prefix-less corpus paths are relative to.
const INTERFACE_DIR: &str = "interface/";

/// `interface\frpvp\frpvp_red.ddj` -> `media://interface/frpvp/frpvp_red.ddj`,
/// and `icon\item\etc\event_news_a.ddj` -> `media://interface/icon/...`.
pub fn art_path(descriptor_path: &str) -> String {
    let normalised = descriptor_path.to_ascii_lowercase().replace('\\', "/");
    if normalised.starts_with(INTERFACE_DIR) {
        format!("{MEDIA_ROOT}{normalised}")
    } else {
        format!("{MEDIA_ROOT}{INTERFACE_DIR}{normalised}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real strings, taken out of the shipped descriptors — not invented ones.
    /// The old conversion turned every one of these into a double `interface/`.
    #[test]
    fn corpus_paths_keep_their_own_interface_root() {
        for raw in [
            r"interface\frpvp\frpvp_frame01.ddj",
            r"interface\ifcommon\com_m_button02.ddj",
            r"interface\ifcommon\bg_tile\com_bg_tile_b.ddj",
            r"interface\guild\gil_list_frame.ddj",
        ] {
            let path = art_path(raw);
            assert!(path.starts_with("media://interface/"), "{raw} -> {path}");
            assert!(
                !path.contains("interface/interface/"),
                "doubled root: {raw} -> {path}"
            );
        }
        assert_eq!(
            art_path(r"interface\frpvp\frpvp_frame02.ddj"),
            "media://interface/frpvp/frpvp_frame02.ddj"
        );
    }

    /// The two corpus paths that are *not* rooted at `interface` — the reason
    /// this is a branch and not a `strip_prefix`.
    #[test]
    fn a_path_outside_interface_still_gets_the_root() {
        assert_eq!(
            art_path(r"icon\item\etc\event_news_a.ddj"),
            "media://interface/icon/item/etc/event_news_a.ddj"
        );
    }

    /// Case and separators come from a Windows corpus; asset ids are neither.
    #[test]
    fn case_and_separators_are_normalised() {
        assert_eq!(
            art_path(r"Interface\IFCommon\COM_Bar02_Right.ddj"),
            "media://interface/ifcommon/com_bar02_right.ddj"
        );
    }
}
