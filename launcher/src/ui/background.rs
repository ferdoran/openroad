use crate::{pk2, utils};
use bevy::asset::io::AssetReader;
use bevy_pk2::prelude::Archive;
use iced::widget::image::Handle as ImageHandle;
use rand::RngExt;
use std::path::Path;

pub fn load_background_sync(media_pk2: &Archive) -> Result<ImageHandle, String> {
    let backgrounds = [
        "launcher/bg_1.dat",
        "launcher/bg_2.dat",
        "launcher/bg_3.dat",
        "launcher_europe/bg_4.dat",
        "launcher_europe/bg_5.dat",
    ];

    let mut rng = rand::rng();
    let bg_index = rng.random_range(0..backgrounds.len());
    let selected_background = backgrounds[bg_index];

    println!("Selected background: {}", selected_background);

    let mut reader = futures_lite::future::block_on(media_pk2.read(Path::new(selected_background)))
        .map_err(|e| {
            let msg = format!("Failed to read {selected_background}: {e}");
            utils::show_error(&msg);
            msg
        })?;

    let mut data = Vec::new();
    futures_lite::future::block_on(async {
        futures_lite::io::AsyncReadExt::read_to_end(&mut reader, &mut data).await
    })
    .map_err(|e| {
        let msg = format!("Failed to read background bytes: {e}");
        utils::show_error(&msg);
        msg
    })?;

    let (w, h, rgba) = pk2::dat_or_image(&data).map_err(|e| {
        utils::show_error(&e);
        e
    })?;
    Ok(ImageHandle::from_rgba(w, h, rgba))
}

pub fn load_exit_image_sync(media_pk2: &Archive) -> Result<ImageHandle, String> {
    let mut reader = futures_lite::future::block_on(media_pk2.read(Path::new("launcher/exit.dat")))
        .map_err(|e| {
            let msg = format!("Failed to read launcher/exit.dat: {e}");
            utils::show_error(&msg);
            msg
        })?;

    let mut data = Vec::new();
    futures_lite::future::block_on(async {
        futures_lite::io::AsyncReadExt::read_to_end(&mut reader, &mut data).await
    })
    .map_err(|e| {
        let msg = format!("Failed to read exit image bytes: {e}");
        utils::show_error(&msg);
        msg
    })?;

    let (w, h, rgba) = pk2::dat_or_image(&data).map_err(|e| {
        utils::show_error(&e);
        e
    })?;
    Ok(ImageHandle::from_rgba(w, h, rgba))
}
