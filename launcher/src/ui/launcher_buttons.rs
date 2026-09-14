use std::path::Path;

use crate::{Message, pk2, utils};
use bevy::asset::io::AssetReader;
use bevy_pk2::prelude::Archive;
use iced::widget::image::Handle as ImageHandle;
use iced::widget::{Image, container, mouse_area, row};
use iced::{Element, Length, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LauncherButtonKind {
    Option,
    Guide,
    Movie,
    Exit,
    Start,
}

#[derive(Debug, Clone)]
pub struct ButtonImages {
    pub normal: ImageHandle,
    pub hover: ImageHandle,
    pub pressed: ImageHandle,
}

#[derive(Debug, Clone)]
pub struct LauncherButtonAssets {
    pub option: ButtonImages,
    pub guide: ButtonImages,
    pub movie: ButtonImages,
    pub exit: ButtonImages,
    pub start: ButtonImages,
}

impl LauncherButtonAssets {
    pub fn for_kind(&self, kind: LauncherButtonKind) -> &ButtonImages {
        match kind {
            LauncherButtonKind::Option => &self.option,
            LauncherButtonKind::Guide => &self.guide,
            LauncherButtonKind::Movie => &self.movie,
            LauncherButtonKind::Exit => &self.exit,
            LauncherButtonKind::Start => &self.start,
        }
    }
}

pub fn load_launcher_buttons_sync(media_pk2: &Archive) -> Result<LauncherButtonAssets, String> {
    let option = load_button_images(media_pk2, "option")?;
    let guide = load_button_images(media_pk2, "guide")?;
    let movie = load_button_images(media_pk2, "movie")?;
    let exit = load_button_images(media_pk2, "exit")?;
    let start = load_button_images(media_pk2, "start")?;

    Ok(LauncherButtonAssets {
        option,
        guide,
        movie,
        exit,
        start,
    })
}

fn load_button_images(media_pk2: &Archive, base: &str) -> Result<ButtonImages, String> {
    let (width, height, rgba) = load_dat_rgba(media_pk2, &format!("launcher/{base}.dat"))?;

    if width % 3 != 0 {
        let msg = format!("launcher/{base}.dat width {width} is not divisible into 3 states");
        utils::show_error(&msg);
        return Err(msg);
    }

    let normal = crop_button_state(width, height, &rgba, 0);
    let hover = crop_button_state(width, height, &rgba, 1);
    let pressed = crop_button_state(width, height, &rgba, 2);

    Ok(ButtonImages {
        normal,
        hover,
        pressed,
    })
}

fn load_dat_rgba(media_pk2: &Archive, path: &str) -> Result<(u32, u32, Vec<u8>), String> {
    let mut reader =
        futures_lite::future::block_on(media_pk2.read(Path::new(path))).map_err(|e| {
            let msg = format!("Failed to read {path}: {e}");
            utils::show_error(&msg);
            msg
        })?;

    let mut data = Vec::new();
    futures_lite::future::block_on(async {
        futures_lite::io::AsyncReadExt::read_to_end(&mut reader, &mut data).await
    })
    .map_err(|e| {
        let msg = format!("Failed to read bytes for {path}: {e}");
        utils::show_error(&msg);
        msg
    })?;

    let (w, h, rgba) = pk2::dat_or_image(&data).map_err(|e| {
        utils::show_error(&e);
        e
    })?;
    Ok((w, h, rgba))
}

fn crop_button_state(width: u32, height: u32, rgba: &[u8], state_index: u32) -> ImageHandle {
    let state_width = width / 3;
    let src_stride = (width * 4) as usize;
    let dst_stride = (state_width * 4) as usize;
    let start_x = (state_index * state_width * 4) as usize;
    let mut state_rgba = Vec::with_capacity((state_width * height * 4) as usize);

    for y in 0..height as usize {
        let start = y * src_stride + start_x;
        state_rgba.extend_from_slice(&rgba[start..start + dst_stride]);
    }

    ImageHandle::from_rgba(state_width, height, state_rgba)
}

pub fn bar(
    assets: &LauncherButtonAssets,
    hovered: Option<LauncherButtonKind>,
    pressed: Option<LauncherButtonKind>,
) -> Element<'static, Message, Theme> {
    let make_button = |kind: LauncherButtonKind| {
        let images = assets.for_kind(kind);
        let handle = if pressed == Some(kind) {
            &images.pressed
        } else if hovered == Some(kind) {
            &images.hover
        } else {
            &images.normal
        };

        mouse_area(
            Image::<ImageHandle>::new(handle.clone())
                .width(Length::Shrink)
                .height(Length::Shrink),
        )
        .on_enter(Message::ButtonHover(kind, true))
        .on_exit(Message::ButtonHover(kind, false))
        .on_press(Message::ButtonPress(kind))
        .on_release(Message::ButtonRelease(kind))
    };

    container(
        row![
            make_button(LauncherButtonKind::Option),
            make_button(LauncherButtonKind::Guide),
            make_button(LauncherButtonKind::Movie),
            make_button(LauncherButtonKind::Exit),
            make_button(LauncherButtonKind::Start),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center),
    )
    .padding(iced::Padding {
        top: 0.0,
        bottom: 0.0,
        left: 0.0,
        right: 0.0,
    })
    .width(Length::Fill)
    .height(Length::Fixed(75.0))
    .into()
}
