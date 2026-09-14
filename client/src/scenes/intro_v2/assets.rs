use bevy::prelude::*;
use bevy_asset_loader::prelude::AssetCollection;

use crate::assets::textdata::Textdata;

/// Assets for the intro v2 scene. Uses the same asset paths as the old
/// intro's collection, so the underlying assets are shared via the
/// asset server; only the collection resource is duplicated because the
/// old one lives in a private module.
#[derive(AssetCollection, Resource)]
#[allow(dead_code)]
pub struct IntroV2Assets {
    // Windows
    #[asset(path = "media://interface/outer/login_window.ddj")]
    pub login_window: Handle<Image>,

    // Logo
    #[asset(path = "media://interface/outer/logo.ddj")]
    pub logo: Handle<Image>,
    #[asset(path = "media://interface/outer/logo-big.ddj")]
    pub logo_big: Handle<Image>,

    // Buttons — the `_europe` variants: `config/define.txt` defines
    // EUROPE_SYSTEM, and `resinfo/pscharacterselect.txt` names
    // `interface\outer\button_europe.ddj` / `info_europe.ddj` directly.
    #[asset(path = "media://interface/outer/button_europe.ddj")]
    pub button: Handle<Image>,
    #[asset(path = "media://interface/outer/button_europe_press.ddj")]
    pub button_press: Handle<Image>,
    #[asset(path = "media://interface/outer/button_europe_focus.ddj")]
    pub button_focus: Handle<Image>,
    // No `button_europe_disable.ddj` ships (`find Media/interface/outer -iname
    // '*europe*disable*'` is empty while `*button_europe*` lists three files),
    // so the disabled frame comes from the shared `button_disable.ddj`. Its DDS
    // header reads 91x40, i.e. the size the intro actually draws these buttons
    // at (`image_button(main_button_style(..), 91.0, 41.0)`); the europe art is
    // 92x40 and gets stretched to the same node either way.
    #[asset(path = "media://interface/outer/button_disable.ddj")]
    pub button_disable: Handle<Image>,
    #[asset(path = "media://interface/outer/list_button.ddj")]
    pub list_button: Handle<Image>,
    #[asset(path = "media://interface/outer/list_button_press.ddj")]
    pub list_button_press: Handle<Image>,
    #[asset(path = "media://interface/outer/list_button_focus.ddj")]
    pub list_button_focus: Handle<Image>,

    // Intro chrome bars — one pair per tree, see `chrome.rs`. All six arts
    // are 1600x172 ARGB1555 and md5-distinct, so none of them substitutes
    // for another.
    // pstitle_europe.txt:729/710 (title: splash, login, server select)
    #[asset(path = "media://interface/outer/blackbar_up_18_europe.ddj")]
    pub title_bar_up: Handle<Image>,
    #[asset(path = "media://interface/outer/blackbar_down_copyright_europe.ddj")]
    pub title_bar_down: Handle<Image>,
    // pscharacterselect_europe.txt:1010/991 (character list + region board)
    #[asset(path = "media://interface/outer/blackbar_up_europe.ddj")]
    pub blackbar_up: Handle<Image>,
    #[asset(path = "media://interface/outer/blackbar_down_europe.ddj")]
    pub blackbar_down: Handle<Image>,
    // pscharactercreate_europe.txt:253/234 (character create — RED)
    #[asset(path = "media://interface/outer/redbar_up_europe.ddj")]
    pub redbar_up: Handle<Image>,
    #[asset(path = "media://interface/outer/redbar_down_europe.ddj")]
    pub redbar_down: Handle<Image>,

    // character_data
    #[asset(path = "media://server_dep/silkroad/textdata/characterdata.txt")]
    pub character_data: Handle<Textdata>,
    // gates the loading state on leveldata.txt so the exp percentage in the
    // character info box never races the textdata parse
    #[asset(path = "media://server_dep/silkroad/textdata/leveldata.txt")]
    pub level_data: Handle<Textdata>,

    // Server List
    #[asset(path = "media://interface/outer/server_window.ddj")]
    pub server_list_window: Handle<Image>,

    #[asset(path = "media://interface/outer/server_up.ddj")]
    pub server_list_slider_button_up: Handle<Image>,
    #[asset(path = "media://interface/outer/server_up_press.ddj")]
    pub server_list_slider_button_up_press: Handle<Image>,
    #[asset(path = "media://interface/outer/server_up_focus.ddj")]
    pub server_list_slider_button_up_focus: Handle<Image>,

    #[asset(path = "media://interface/outer/server_down.ddj")]
    pub server_list_slider_button_down: Handle<Image>,
    #[asset(path = "media://interface/outer/server_down_press.ddj")]
    pub server_list_slider_button_down_press: Handle<Image>,
    #[asset(path = "media://interface/outer/server_down_focus.ddj")]
    pub server_list_slider_button_down_focus: Handle<Image>,

    #[asset(path = "media://interface/outer/server_mov.ddj")]
    pub server_list_slider_button_mov: Handle<Image>,
    #[asset(path = "media://interface/outer/server_mov_press.ddj")]
    pub server_list_slider_button_mov_press: Handle<Image>,
    #[asset(path = "media://interface/outer/server_mov_focus.ddj")]
    pub server_list_slider_button_mov_focus: Handle<Image>,

    #[asset(path = "media://interface/outer/server_rollover.ddj")]
    pub server_list_item_hover: Handle<Image>,
    #[asset(path = "media://interface/outer/server_select.ddj")]
    pub server_list_item_select: Handle<Image>,

    // Character selection
    #[asset(path = "media://interface/outer/info_europe.ddj")]
    pub info_window: Handle<Image>,
    #[asset(path = "media://interface/outer/hp.ddj")]
    pub hp_bar: Handle<Image>,
    #[asset(path = "media://interface/outer/mp.ddj")]
    pub mp_bar: Handle<Image>,
    #[asset(path = "media://interface/outer/warning_delete.ddj")]
    pub warning_delete_window: Handle<Image>,
    /// `GDR_STA_WCREATE` (`pscharactercreate{china,_europe}.txt:25-43`), the
    /// create-confirm modal's 248x128 frame — the create screen's sibling of
    /// `warning_delete.ddj`, which char-select already uses.
    #[asset(path = "media://interface/outer/warning_create.ddj")]
    pub warning_create_window: Handle<Image>,
    #[asset(path = "media://interface/outer/warning_button.ddj")]
    pub warning_button: Handle<Image>,
    #[asset(path = "media://interface/outer/warning_button_focus.ddj")]
    pub warning_button_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/warning_button_press.ddj")]
    pub warning_button_press: Handle<Image>,

    // Character create chrome — the panels and buttons the screen's own
    // resinfo trees name. Both race trees are read: `pscharactercreatechina`
    // and `pscharactercreate_europe` diverge on exactly five assets, and
    // `customize_window` / `explain-window` are two of them (:139-157 and
    // :101-119 in either file).
    #[asset(path = "media://interface/outer/customize_window.ddj")]
    pub customize_window: Handle<Image>,
    #[asset(path = "media://interface/outer/customize_window_europe.ddj")]
    pub customize_window_europe: Handle<Image>,
    #[asset(path = "media://interface/outer/explain-window.ddj")]
    pub explain_window: Handle<Image>,
    #[asset(path = "media://interface/outer/explain-window_02.ddj")]
    pub explain_window_02: Handle<Image>,
    // `GDR_BTN_MALE` / `GDR_BTN_FEMALE` (:390-408 / :371-389): the selected
    // state is a TEXTURE swap (`man_on` <-> `man_off`), not a text colour —
    // the data gives both captions the same `255,249,212`.
    #[asset(path = "media://interface/outer/man_on.ddj")]
    pub man_on: Handle<Image>,
    #[asset(path = "media://interface/outer/man_on_focus.ddj")]
    pub man_on_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/man_on_press.ddj")]
    pub man_on_press: Handle<Image>,
    #[asset(path = "media://interface/outer/man_off.ddj")]
    pub man_off: Handle<Image>,
    #[asset(path = "media://interface/outer/man_off_focus.ddj")]
    pub man_off_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/man_off_press.ddj")]
    pub man_off_press: Handle<Image>,
    #[asset(path = "media://interface/outer/woman_on.ddj")]
    pub woman_on: Handle<Image>,
    #[asset(path = "media://interface/outer/woman_on_focus.ddj")]
    pub woman_on_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/woman_on_press.ddj")]
    pub woman_on_press: Handle<Image>,
    #[asset(path = "media://interface/outer/woman_off.ddj")]
    pub woman_off: Handle<Image>,
    #[asset(path = "media://interface/outer/woman_off_focus.ddj")]
    pub woman_off_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/woman_off_press.ddj")]
    pub woman_off_press: Handle<Image>,
    // `GDR_BTN_CHECK` (:409-427) — an exact-fit 75x25 texture, so the screen
    // stops borrowing the 91x30 generic login button for it.
    #[asset(path = "media://interface/outer/overlap.ddj")]
    pub overlap: Handle<Image>,
    #[asset(path = "media://interface/outer/overlap_focus.ddj")]
    pub overlap_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/overlap_press.ddj")]
    pub overlap_press: Handle<Image>,

    // `Section = Slider` (`:685-744`) — ONE reusable template that serves all
    // five option rows: the thumb and the two arrows, each with its
    // `_focus`/`_press` variants.
    #[asset(path = "media://interface/outer/slider.ddj")]
    pub slider_thumb: Handle<Image>,
    #[asset(path = "media://interface/outer/slider_focus.ddj")]
    pub slider_thumb_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/slider_press.ddj")]
    pub slider_thumb_press: Handle<Image>,
    #[asset(path = "media://interface/outer/arrow_left.ddj")]
    pub slider_arrow_left: Handle<Image>,
    #[asset(path = "media://interface/outer/arrow_left_focus.ddj")]
    pub slider_arrow_left_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/arrow_left_press.ddj")]
    pub slider_arrow_left_press: Handle<Image>,
    #[asset(path = "media://interface/outer/arrow_right.ddj")]
    pub slider_arrow_right: Handle<Image>,
    #[asset(path = "media://interface/outer/arrow_right_focus.ddj")]
    pub slider_arrow_right_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/arrow_right_press.ddj")]
    pub slider_arrow_right_press: Handle<Image>,

    // Character create — `Section = Rotate` of
    // `resinfo/pscharactercreate{china,_europe}.txt` (both trees byte-identical
    // here, :120-138 for the window and :626-682 for the three buttons). Every
    // button ships `_focus`/`_press`, so they use the same three-state style as
    // the rest of the intro.
    #[asset(path = "media://interface/outer/rotate_window.ddj")]
    pub rotate_window: Handle<Image>,
    #[asset(path = "media://interface/outer/rotate_left.ddj")]
    pub rotate_left: Handle<Image>,
    #[asset(path = "media://interface/outer/rotate_left_focus.ddj")]
    pub rotate_left_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/rotate_left_press.ddj")]
    pub rotate_left_press: Handle<Image>,
    #[asset(path = "media://interface/outer/rotate_right.ddj")]
    pub rotate_right: Handle<Image>,
    #[asset(path = "media://interface/outer/rotate_right_focus.ddj")]
    pub rotate_right_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/rotate_right_press.ddj")]
    pub rotate_right_press: Handle<Image>,
    #[asset(path = "media://interface/outer/zoomin.ddj")]
    pub zoomin: Handle<Image>,
    #[asset(path = "media://interface/outer/zoomin_focus.ddj")]
    pub zoomin_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/zoomin_press.ddj")]
    pub zoomin_press: Handle<Image>,
    #[asset(path = "media://interface/outer/zoomout.ddj")]
    pub zoomout: Handle<Image>,
    #[asset(path = "media://interface/outer/zoomout_focus.ddj")]
    pub zoomout_focus: Handle<Image>,
    #[asset(path = "media://interface/outer/zoomout_press.ddj")]
    pub zoomout_press: Handle<Image>,

    // World join loading overlay
    #[asset(path = "media://interface/loading/loading_default.ddj")]
    pub loading_background: Handle<Image>,

    // Captcha
    #[asset(path = "media://interface/outer/imagecode_window.ddj")]
    pub captcha_window: Handle<Image>,
    #[asset(path = "media://interface/ifcommon/com_mid_button.ddj")]
    pub captcha_confirm_button: Handle<Image>,
    #[asset(path = "media://interface/ifcommon/com_mid_button_focus.ddj")]
    pub captcha_confirm_button_focus: Handle<Image>,
    #[asset(path = "media://interface/ifcommon/com_mid_button_press.ddj")]
    pub captcha_confirm_button_press: Handle<Image>,

    // Sounds
    #[asset(path = "data://prim/snd/ui/uibutton_a.wav")]
    pub sound_button_sound_a: Handle<AudioSource>,
    #[asset(path = "data://prim/snd/ui/uiwinopen.wav")]
    pub sound_window_open: Handle<AudioSource>,
    #[asset(path = "data://prim/snd/ui/uiwinclose.wav")]
    pub sound_window_close: Handle<AudioSource>,
    #[asset(path = "data://prim/snd/ui/error.wav")]
    pub sound_error: Handle<AudioSource>,
}
