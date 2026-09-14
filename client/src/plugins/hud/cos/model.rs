//! COS (companion) window state: open/closed plus which of the three pages
//! the shell shows.
//!
//! Idea: vanilla's COS shell (`resinfo/ginterface.txt:1330`, `GDR_COS_WND`,
//! id 120) hosts three page controls that all share one rect
//! (`resinfo/ifcos.txt`: `GDR_COS_INFO` id 121, `GGDR_COS_INVENTORY` id 122,
//! `GDR_COS_SETUP` id 123, each `Rect="12,66,331,314"`). The resinfo grammar
//! has no `Visible` key, so "one page at a time" is the inferred reading of
//! that shared rect (`docs/re/ui/cos-pet-window.md` §3, marked `[S]`) and the
//! selection lives here in code, exactly as it does in the original.
//! The window is opened by the vanilla `KeyCOSInfo` shortcut (OptionSet.csv id
//! 3016) — unbound by default, like the other 27 shortcuts whose vanilla
//! default we do not have (`settings/keymap.rs`).

use bevy::prelude::*;

use crate::plugins::hud::chat::model::ChatState;
use crate::plugins::settings::keymap::KEY_COS_INFO;
use crate::plugins::settings::options::GameOptions;

/// The shell's three pages, in the order `ifcos.txt` declares them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CosPage {
    /// `GDR_COS_INFO:CIFCOSInfo`, id 121 — `ifcosinfo.txt`.
    #[default]
    Info,
    /// `GGDR_COS_INVENTORY:CIFCOSInventory`, id 122 — `ifcosinventory.txt`.
    Inventory,
    /// `GDR_COS_SETUP:CIFCOSSetup`, id 123 — `ifcossetup.txt`.
    Setup,
}

/// Open/closed state and page selection of the COS window.
#[derive(Resource, Default)]
pub struct CosWindowState {
    pub open: bool,
    pub page: CosPage,
}

/// Toggle with the `KeyCOSInfo` shortcut (unless the chat input is capturing
/// keys). Unbound until the user binds it, so this is inert by default.
pub fn toggle_cos_window(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatState>,
    options: Res<GameOptions>,
    mut state: ResMut<CosWindowState>,
) {
    let Some(key) = options.key_for(KEY_COS_INFO) else {
        return;
    };
    if keys.just_pressed(key) && !chat.input_open {
        state.open = !state.open;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// The declaration order in `ifcos.txt` is info -> inventory -> setup, and
    /// info is the page the shortcut's own name (`KeyCOSInfo`) opens.
    #[test]
    fn default_page_is_the_info_page() {
        assert_eq!(CosPage::default(), CosPage::Info);
    }
}
