use std::io::Read;

use bevy::asset::RenderAssetUsages;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::text::{EditableText, EditableTextFilter, FontSourceTemplate};
use bevy::ui_widgets::Activate;
use image::EncodableLayout;
use libflate::zlib::Decoder;

use packets::login::{
    LoginCaptchaChallenge, LoginCaptchaConfirmRequest, LoginCaptchaConfirmResponse,
};
use packets::Packet;

use crate::assets::FontAssets;
use crate::net::connection::SilkroadConnection;
use crate::plugins::hud::modal_dialog::{modal_plate, modal_scrim};
use crate::plugins::net::gateway::GatewayConnection;
use crate::plugins::settings::options::GameOptions;
use crate::plugins::textdata::ClientUiStrings;
use crate::plugins::ui_v2::style::ImageButtonStyle;
use crate::plugins::ui_v2::widgets::{image_button, label, text_input};

use super::assets::IntroV2Assets;
use super::chrome::InfoTextV2Update;
use super::IntroV2Ui;

/// Upper bound on the inflated bitmap we are willing to expand, so a hostile or
/// broken server cannot make us allocate arbitrarily. 64x the stock IBUV image,
/// which is 200x64 at 1bpp = 1600 bytes (`docs/net-login-gateway.md`).
const MAX_DECOMPRESSED_IMAGE_SIZE: usize = (200 * 64 / 8) * 64;

/// Expand 0x2322's payload into RGBA pixels.
///
/// The blob is a zlib stream of a 1-bit-per-pixel bitmap, LSB first within each
/// byte, white on opaque black. The inflated length is **derived** from the
/// packet's own `image_width`/`image_height` as `width * height / 8`, so a server
/// sending a differently sized image renders instead of crashing the client.
///
/// It is deliberately *not* taken from the packet's `unk_0x32c8` field: that
/// field is a constant `0x32C8` (13000) in all five `packet_dump/0x2322.log`
/// samples while the payloads inflate to exactly 1600 bytes at 200x64, so it does
/// not carry the size despite its old name (`docs/net-login-gateway.md`).
///
/// Returns `None` for any payload whose dimensions or inflated length do not
/// agree — the modal then simply does not appear.
fn decode_captcha_pixels(challenge: &LoginCaptchaChallenge) -> Option<Vec<u8>> {
    let width = challenge.image_width as usize;
    let height = challenge.image_height as usize;

    if width == 0 || height == 0 {
        warn!("captcha: degenerate image size {width}x{height}");
        return None;
    }
    if (width * height) % 8 != 0 {
        warn!("captcha: {width}x{height} is not a whole number of 1bpp bytes");
        return None;
    }
    let uncompressed = width * height / 8;
    if uncompressed > MAX_DECOMPRESSED_IMAGE_SIZE {
        warn!("captcha: refusing {uncompressed}-byte bitmap");
        return None;
    }

    let mut inflated = Vec::with_capacity(uncompressed);
    let decoder = match Decoder::new(challenge.image_data.as_bytes()) {
        Ok(decoder) => decoder,
        Err(e) => {
            warn!("captcha: not a zlib stream: {e}");
            return None;
        }
    };
    if let Err(e) = decoder.take(uncompressed as u64).read_to_end(&mut inflated) {
        warn!("captcha: failed to inflate image: {e}");
        return None;
    }
    if inflated.len() != uncompressed {
        warn!(
            "captcha: inflated {} bytes, header says {uncompressed}",
            inflated.len()
        );
        return None;
    }

    let mut pixel_data = Vec::with_capacity(width * height * 4);
    for byte in inflated {
        for bit in 0..8 {
            if byte & (1 << bit) > 0 {
                pixel_data.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]);
            } else {
                pixel_data.extend_from_slice(&[0, 0, 0, 0xff]);
            }
        }
    }

    Some(pixel_data)
}

/// textuisystem keys the original's `ifconfirmbox.txt` binds to this dialog's
/// three labels (`GDR_STA_CAPTION:78`, `GDR_STA_EXPLAIN:116`,
/// `GDR_BTN_ACCEPT:59`); the fallbacks mirror the shipped English rows.
const CAPTION_KEY: &str = "UIIT_PAG_GLOBAL_AUTHENTICATION";
const NOTICE_KEY: &str = "UIIT_STT_GLOBAL_AUTHENTICATION_NOTICE";
const CONFIRM_KEY: &str = "UIIS_CTL_CONFIRM";

/// Native size of the captcha box art (`ifconfirmbox.txt`'s plate). Unchanged
/// by #664 — the shared shell owns the chrome, not this dialog's geometry.
const CAPTCHA_BOX_W: f32 = 400.0;
const CAPTCHA_BOX_H: f32 = 180.0;

/// The decoded captcha image; its presence triggers the modal to spawn.
#[derive(Resource)]
pub struct CaptchaImageV2(pub Handle<Image>);

/// Root marker of the captcha modal.
#[derive(Component, Default, Clone)]
pub struct CaptchaModal;

/// Marker on the captcha code `EditableText`.
#[derive(Component, Default, Clone)]
pub struct CaptchaInput;

pub fn on_captcha_challenge(
    mut events: MessageReader<LoginCaptchaChallenge>,
    mut commands: Commands,
    mut image_assets: ResMut<Assets<Image>>,
) {
    for event in events.read() {
        let Some(pixel_data) = decode_captcha_pixels(event) else {
            continue;
        };

        let img = Image::new_fill(
            Extent3d {
                width: event.image_width as u32,
                height: event.image_height as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &pixel_data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );

        let image_handle = image_assets.add(img);

        commands.insert_resource(CaptchaImageV2(image_handle));
    }
}

fn confirm_button_style(assets: &IntroV2Assets) -> ImageButtonStyle {
    ImageButtonStyle {
        normal: assets.captcha_confirm_button.clone(),
        hover: assets.captcha_confirm_button_focus.clone(),
        press: assets.captcha_confirm_button_press.clone(),
        ..Default::default()
    }
}

fn captcha_modal(
    captcha_image: Handle<Image>,
    assets: &IntroV2Assets,
    fonts: &FontAssets,
    ui_strings: &ClientUiStrings,
) -> impl Scene {
    let caption = ui_strings
        .get_or(CAPTION_KEY, "Image code verification")
        .to_string();
    // The notice row is authored for the original's `CIFPML` rich-text control,
    // so it carries markup; `get_plain_or` reduces it (see `textdata::plain_text`).
    let notice = ui_strings.get_plain_or(
        NOTICE_KEY,
        "To prevent auto creation,\nplease enter the number/text as it appears.",
    );
    let confirm = ui_strings.get_or(CONFIRM_KEY, "Confirm").to_string();
    let window = assets.captcha_window.clone();
    let caption_font = fonts.nine.clone();
    let description_font = fonts.nine.clone();
    let input_font = fonts.nine.clone();
    let confirm_font = fonts.nine.clone();
    let confirm_sound = assets.sound_error.clone();

    // The host entry `pstitle.txt:6` `GDR_CMB_CONFIRMBOX:CIFConfirmbox` (id 47,
    // section `Validate`) has a degenerate `Rect="0,0,1,1"`, so the original
    // positions this box from code and the data says nothing about where it goes
    // (`docs/re/ui/scene-intro-captcha.md` §3/§9). Centring it over a dimmed
    // screen is therefore an openroad convention, not a reproduction; only the
    // 400x180 box art and the child rects below come from the original.
    bsn! {
        modal_scrim()
        CaptchaModal
        Name("Captcha Modal V2")
        TabGroup::new(0)
        Children [
            (
                modal_plate(window, CAPTCHA_BOX_W, CAPTCHA_BOX_H)
                Children [
                    (
                        Text({caption})
                        TextFont { font: FontSourceTemplate::Handle({caption_font}), font_size: {FontSize::Px(12.0)} }
                        TextColor(Color::WHITE)
                        Node {
                            position_type: PositionType::Absolute,
                            align_self: AlignSelf::Center,
                            width: px(229),
                            height: px(15),
                            left: px(85),
                            top: px(18),
                        }
                    ),
                    (
                        Text({notice})
                        TextFont { font: FontSourceTemplate::Handle({description_font}), font_size: {FontSize::Px(12.0)} }
                        TextColor(Color::WHITE)
                        Node {
                            position_type: PositionType::Absolute,
                            align_self: AlignSelf::Center,
                            width: px(336),
                            height: px(40),
                            left: px(32),
                            top: px(40),
                        }
                    ),
                    (
                        ImageNode { image: {captcha_image} }
                        Node {
                            position_type: PositionType::Absolute,
                            width: px(200),
                            height: px(64),
                            top: px(80),
                            left: px(33),
                        }
                    ),
                    (
                        text_input(input_font, 0)
                        CaptchaInput
                        EditableTextFilter::new(char::is_alphanumeric)
                        Node {
                            position_type: PositionType::Absolute,
                            align_self: AlignSelf::Center,
                            width: px(95),
                            height: px(15),
                            left: px(260),
                            top: px(88),
                        }
                    ),
                    (
                        image_button(confirm_button_style(assets), 55.0, 18.0)
                        Node {
                            position_type: PositionType::Absolute,
                            width: px(55),
                            height: px(18),
                            top: px(121),
                            left: px(257),
                        }
                        Children [
                            // label() starts transparent for the fade systems,
                            // which never run on this modal, so force it white.
                            (
                                label(&confirm, confirm_font, 12.0)
                                TextColor(Color::WHITE)
                            ),
                        ]
                        on(move |_activate: On<Activate>,
                            connection_query: Query<&SilkroadConnection, With<GatewayConnection>>,
                            input_query: Query<&EditableText, With<CaptchaInput>>,
                            modal_query: Query<Entity, With<CaptchaModal>>,
                            options: Res<GameOptions>,
                            mut commands: Commands| {
                            let Ok(connection) = connection_query.single() else {
                                return;
                            };
                            let Ok(input) = input_query.single() else {
                                return;
                            };

                            let frame = Packet::from(LoginCaptchaConfirmRequest {
                                code: input.value().to_string(),
                            })
                            .into();

                            connection
                                .get_sender()
                                .send(frame)
                                .expect("failed to send LoginCaptchaConfirmRequest");

                            commands.remove_resource::<CaptchaImageV2>();
                            if let Ok(modal) = modal_query.single() {
                                commands.entity(modal).despawn();
                            }
                            if let Some(playback) = options.audio.fx_playback() {
                                commands.spawn((
                                    AudioPlayer::new(confirm_sound.clone()),
                                    playback,
                                ));
                            }
                        })
                    ),
                ]
            ),
        ]
    }
}

pub fn spawn_captcha(
    captcha: Res<CaptchaImageV2>,
    assets: Res<IntroV2Assets>,
    fonts: Res<FontAssets>,
    ui_strings: Res<ClientUiStrings>,
    mut commands: Commands,
    cam_query: Query<Entity, With<Camera2d>>,
) {
    let Some(camera) = cam_query.iter().next() else {
        warn!("no 2d camera found");
        return;
    };

    commands
        .spawn_scene(captcha_modal(
            captcha.0.clone(),
            &assets,
            &fonts,
            &ui_strings,
        ))
        .insert((UiTargetCamera(camera), IntroV2Ui));
}

pub fn on_captcha_confirm_response(
    mut events: MessageReader<LoginCaptchaConfirmResponse>,
    mut info_text_writer: MessageWriter<InfoTextV2Update>,
    mut commands: Commands,
    assets: Res<IntroV2Assets>,
    options: Res<GameOptions>,
) {
    for event in events.read() {
        match &event.wrong_attempt {
            None => continue,
            Some(wrong_attempt) => {
                info_text_writer.write(InfoTextV2Update(format!(
                    "Image code entry has failed {} out of {} times.",
                    wrong_attempt.cur_attempts, wrong_attempt.max_attempts
                )));
                if let Some(playback) = options.audio.fx_playback() {
                    commands.spawn((AudioPlayer::new(assets.sound_error.clone()), playback));
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use std::io::Write;

    use libflate::zlib::Encoder;

    use super::*;
    use crate::assets::textdata::uisystem::UiSystemText;
    use crate::plugins::textdata::plain_text;

    /// A 0x2322 payload in the stock shape the gateway sends: a zlib stream of
    /// `width * height / 8` bytes at 1bpp. `unk_0x32c8` carries the constant the
    /// real captures carry, so a decoder that still trusted it would fail here
    /// (`docs/net-login-gateway.md`).
    fn challenge(width: u16, height: u16, bitmap: &[u8]) -> LoginCaptchaChallenge {
        let mut encoder = Encoder::new(Vec::new()).expect("zlib encoder");
        encoder.write_all(bitmap).expect("deflate");
        let image_data = encoder.finish().into_result().expect("zlib stream");

        LoginCaptchaChallenge {
            // Header values as captured: flag 0, remain = everything after it.
            image_flag: 0,
            image_remain: (image_data.len() + 8) as u16,
            image_compressed: image_data.len() as u16,
            // The observed constant, which the decoder must ignore.
            unk_0x32c8: 0x32C8,
            image_width: width,
            image_height: height,
            image_data,
        }
    }

    #[test]
    fn decode_captcha_pixels_expands_the_stock_200x64_image() {
        // 1600 bytes: first byte 0b0000_0001 -> only pixel 0 white, rest black.
        let mut bitmap = vec![0u8; 200 * 64 / 8];
        bitmap[0] = 0x01;

        let pixels =
            decode_captcha_pixels(&challenge(200, 64, &bitmap)).expect("stock captcha decodes");

        assert_eq!(pixels.len(), 200 * 64 * 4);
        assert_eq!(&pixels[0..4], &[0xff, 0xff, 0xff, 0xff]);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 0xff]);
    }

    #[test]
    fn decode_captcha_pixels_follows_the_wire_size_not_the_stock_size() {
        let bitmap = vec![0xffu8; 8]; // 8 bytes = 64 pixels = 8x8
        let pixels =
            decode_captcha_pixels(&challenge(8, 8, &bitmap)).expect("non-stock captcha decodes");

        assert_eq!(pixels.len(), 8 * 8 * 4);
        assert!(pixels
            .chunks(4)
            .all(|p| p == [0xff, 0xff, 0xff, 0xff].as_slice()));
    }

    #[test]
    fn decode_captcha_pixels_rejects_inconsistent_or_corrupt_payloads() {
        let bitmap = vec![0u8; 200 * 64 / 8];

        // degenerate dimensions
        assert!(decode_captcha_pixels(&challenge(0, 64, &bitmap)).is_none());
        // dimensions that are not a whole number of 1bpp bytes
        assert!(decode_captcha_pixels(&challenge(3, 1, &bitmap)).is_none());
        // truncated / non-zlib blob
        let mut corrupt = challenge(200, 64, &bitmap);
        corrupt.image_data = vec![0xde, 0xad, 0xbe, 0xef];
        assert!(decode_captcha_pixels(&corrupt).is_none());
        // a stream that inflates to fewer bytes than the dimensions require
        assert!(decode_captcha_pixels(&challenge(200, 64, &bitmap[..800])).is_none());
    }

    /// Rows :11, :3968 and :3969 verbatim from the user's
    /// `media://server_dep/silkroad/textdata/textuisystem.txt` — 10 tab-separated
    /// columns with the English text last.
    const TEXTUISYSTEM_ROWS: &str = concat!(
        "1\tUIIS_CTL_CONFIRM\t\t\t\t\t\t\t\tConfirm\r\n",
        "1\tUIIT_PAG_GLOBAL_AUTHENTICATION\t\t\t\t\t\t\t\tImage code verification\r\n",
        "1\tUIIT_STT_GLOBAL_AUTHENTICATION_NOTICE\t\t\t\t\t\t\t\t",
        "<sml2>To prevent auto creation,<br>please enter the number/text as it appears.</sml2>\r\n",
    );

    #[test]
    fn captcha_keys_resolve_against_the_shipped_string_table() {
        let strings = UiSystemText::parse(TEXTUISYSTEM_ROWS);

        assert_eq!(strings.get(CAPTION_KEY), Some("Image code verification"));
        assert_eq!(strings.get(CONFIRM_KEY), Some("Confirm"));
        assert_eq!(
            plain_text(strings.get(NOTICE_KEY).expect("notice row")),
            "To prevent auto creation,\nplease enter the number/text as it appears."
        );
    }

    #[test]
    fn plain_text_keeps_authored_breaks_and_drops_tags() {
        assert_eq!(
            plain_text("<font color=\"255,0,0,0\">red<br/>line</font>"),
            "red\nline"
        );
        assert_eq!(plain_text("no markup"), "no markup");
        assert_eq!(plain_text("unterminated <tag"), "unterminated <tag");
    }
}
