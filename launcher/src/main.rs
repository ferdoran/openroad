use iced::widget::image::Handle as ImageHandle;
use iced::widget::text as text_widget;
use iced::widget::{Column, Image, Row, button, column, container, mouse_area, stack, text};
use iced::{Alignment, Color, ContentFit, Element, Length, Size, Task, Theme, exit, window};

use iced::widget::markdown;
use serde::Deserialize;

use std::env;
use std::path::Path;

use bevy_pk2::prelude::Archive;

mod news;
mod pk2;
mod ui;
mod utils;
mod version;
use crate::ui::background::{load_background_sync, load_exit_image_sync};
use crate::ui::header::header_links;
use crate::ui::launcher_buttons::{
    LauncherButtonAssets, LauncherButtonKind, load_launcher_buttons_sync,
};

use crate::ui::launcher_buttons;
use ui::{NewsItem, news_list::news_list, news_preview::news_preview};

const WINDOW_W: f32 = 755.0;
const WINDOW_H: f32 = 469.0;

pub fn main() -> iced::Result {
    let media_pk2 = match pk2::read_pk2("Media.pk2") {
        Ok(archive) => archive,
        Err(e) => {
            println!("Konnte Media.pk2 nicht laden: {e}");
            return Ok(());
        }
    };

    if let Err(e) = preflight(&media_pk2) {
        println!("{e}");
    }

    // Load .env from CWD first, then from the launcher crate root.
    let _ = dotenvy::dotenv();
    if env::var("DISCORD_URL").is_err() && env::var("YOUTUBE_URL").is_err() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let env_path = Path::new(manifest_dir).join(".env");
        let _ = dotenvy::from_path(env_path);
    }

    // Temporary diagnostics: print when URLs are missing.
    if env::var("DISCORD_URL").is_err() || env::var("YOUTUBE_URL").is_err() {
        eprintln!(
            "Launcher env missing. DISCORD_URL={:?} YOUTUBE_URL={:?}",
            env::var("DISCORD_URL"),
            env::var("YOUTUBE_URL")
        );
    }

    iced::application("OpenRoad Launcher", update, view)
        .window(window::Settings {
            size: Size::new(WINDOW_W, WINDOW_H),
            resizable: false,
            decorations: false,
            ..Default::default()
        })
        .run_with(move || {
            (
                Launcher::default(),
                Task::batch(vec![
                    Task::done(load_background_sync(&media_pk2)).map(Message::BackgroundLoaded),
                    Task::done(load_exit_image_sync(&media_pk2)).map(Message::ExitImageLoaded),
                    Task::done(load_launcher_buttons_sync(&media_pk2)).map(Message::ButtonsLoaded),
                    Task::perform(news::load_from_env(), Message::NewsLoaded),
                ]),
            )
        })
}

fn preflight(media_pk2: &Archive) -> Result<(), String> {
    let version = version::read_sv_t_version(media_pk2)?;
    println!("Gefundene Version in SV.T: {}", version);
    Ok(())
}

struct Launcher {
    selected_news: usize,
    background: Option<ImageHandle>,
    exit_image: Option<ImageHandle>,
    buttons: Option<LauncherButtonAssets>,
    hovered_button: Option<LauncherButtonKind>,
    pressed_button: Option<LauncherButtonKind>,
    error: Option<String>,
    news: Vec<NewsItem>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            selected_news: 0,
            background: None,
            exit_image: None,
            buttons: None,
            hovered_button: None,
            pressed_button: None,
            error: None,
            news: news::default(),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // variants are constructed once the launcher UI wires up its buttons
enum Message {
    Start,
    Exit,
    OpenSite,
    OpenCoffee,
    OpenDiscord,
    OpenYouTube,
    SelectNews(usize),
    DragRequest,
    DragWindow(Option<window::Id>),
    BackgroundLoaded(Result<ImageHandle, String>),
    ExitImageLoaded(Result<ImageHandle, String>),
    ButtonsLoaded(Result<LauncherButtonAssets, String>),
    ButtonHover(LauncherButtonKind, bool),
    ButtonPress(LauncherButtonKind),
    ButtonRelease(LauncherButtonKind),
    NewsLoaded(Result<Vec<NewsItem>, String>),
    OpenNewsLink(markdown::Url),
}

const MAX_NEWS: usize = 6;

#[derive(Debug, Deserialize)]
struct NewsIndexItem {
    timestamp: String,
    title: String,
    md: String,
}

fn update(state: &mut Launcher, message: Message) -> Task<Message> {
    let _ = state;
    match message {
        Message::Start => {
            // TODO: launch the game client process.
            Task::none()
        }
        Message::Exit => exit(),
        Message::OpenSite => {
            match utils::get_env_url("SITE_URL") {
                Some(url) => utils::open_url(url.as_str()),
                None => utils::show_error("DISCORD_URL is not set."),
            }
            Task::none()
        }
        Message::OpenCoffee => {
            match utils::get_env_url("COFFEE_URL") {
                Some(url) => utils::open_url(url.as_str()),
                None => utils::show_error("DISCORD_URL is not set."),
            }
            Task::none()
        }
        Message::OpenDiscord => {
            match utils::get_env_url("DISCORD_URL") {
                Some(url) => utils::open_url(url.as_str()),
                None => utils::show_error("DISCORD_URL is not set."),
            }
            Task::none()
        }
        Message::OpenYouTube => {
            match utils::get_env_url("YOUTUBE_URL") {
                Some(url) => utils::open_url(url.as_str()),
                None => utils::show_error("YOUTUBE_URL is not set."),
            }
            Task::none()
        }
        Message::SelectNews(index) => {
            state.selected_news = index.min(state.news.len().saturating_sub(1));
            Task::none()
        }
        Message::DragRequest => window::get_latest().map(Message::DragWindow),
        Message::DragWindow(Some(id)) => window::drag(id),
        Message::DragWindow(None) => Task::none(),
        Message::BackgroundLoaded(result) => {
            match result {
                Ok(handle) => {
                    state.background = Some(handle);
                }
                Err(err) => {
                    state.error = Some(err);
                    return exit();
                }
            }
            Task::none()
        }
        Message::ExitImageLoaded(result) => {
            if let Ok(handle) = result {
                state.exit_image = Some(handle);
            }
            Task::none()
        }
        Message::ButtonsLoaded(result) => {
            if let Ok(assets) = result {
                state.buttons = Some(assets);
            }
            Task::none()
        }
        Message::ButtonHover(kind, is_hovered) => {
            if is_hovered {
                state.hovered_button = Some(kind);
            } else if state.hovered_button == Some(kind) {
                state.hovered_button = None;
            }
            Task::none()
        }
        Message::ButtonPress(kind) => {
            state.pressed_button = Some(kind);
            Task::none()
        }
        Message::ButtonRelease(kind) => {
            if state.pressed_button == Some(kind) {
                state.pressed_button = None;
                match kind {
                    LauncherButtonKind::Option => {
                        utils::show_error("Option is not implemented yet.");
                    }
                    LauncherButtonKind::Guide => {
                        utils::show_error("Guide is not implemented yet.");
                    }
                    LauncherButtonKind::Movie => {
                        utils::show_error("Movie is not implemented yet.");
                    }
                    LauncherButtonKind::Exit => {
                        return exit();
                    }
                    LauncherButtonKind::Start => {
                        utils::show_error("Start is not implemented yet.");
                    }
                }
            }
            Task::none()
        }
        Message::NewsLoaded(result) => {
            match result {
                Ok(news) => {
                    state.news = news;
                    state.selected_news = 0;
                    state.error = None;
                }
                Err(err) => {
                    state.error = Some(err);
                    if state.news.is_empty() {
                        state.news = news::default();
                    }
                }
            }
            Task::none()
        }
        Message::OpenNewsLink(url) => {
            utils::open_url(url.as_str());
            Task::none()
        }
    }
}

fn view(state: &Launcher) -> Element<'_, Message, Theme> {
    const LEFT_W: f32 = 370.0;

    if state.news.is_empty() {
        let mut content = column![text("No news available.").size(14)];
        if let Some(err) = &state.error {
            content = content.push(text(err).size(12).style(|_| text_widget::Style {
                color: Some(Color::from_rgb8(240, 200, 160)),
            }));
        } else {
            content = content.push(text("News feed is not available.").size(12).style(|_| {
                text_widget::Style {
                    color: Some(Color::from_rgb8(240, 200, 160)),
                }
            }));
        }
        return container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
    }

    let selected = state.selected_news.min(state.news.len().saturating_sub(1));
    let items = state.news.as_slice();
    let selected_item = items
        .get(selected)
        .unwrap_or_else(|| items.first().expect("news list should not be empty"));
    let mut notice_panel = column![];
    if let Some(err) = &state.error {
        notice_panel = notice_panel.push(text(err).size(12).style(|_| text_widget::Style {
            color: Some(Color::from_rgb8(240, 200, 160)),
        }));
    }
    let preview_height = (WINDOW_H - (6.0 * 2.0) - 200.0).max(120.0);
    notice_panel = notice_panel
        .push(news_list(items, selected, Length::Fixed(LEFT_W)))
        .push(news_preview(
            selected_item,
            Length::Fixed(LEFT_W),
            Length::Fixed(preview_height),
        ))
        .spacing(0)
        .width(Length::Fixed(LEFT_W))
        .height(Length::Fill);

    let right_column = column![
        container(header_links())
            .width(Length::Shrink)
            .align_x(Alignment::End),
    ]
    .spacing(6)
    .align_x(Alignment::End);

    let top_row: Row<Message, Theme> = Row::new()
        .push(container(notice_panel).padding(iced::Padding {
            top: 5.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }))
        .push(container(right_column).width(Length::Shrink))
        .spacing(30)
        .width(Length::Fill);

    let buttons_bar = if let Some(assets) = &state.buttons {
        launcher_buttons::bar(assets, state.hovered_button, state.pressed_button)
    } else {
        container(text(""))
            .width(Length::Fill)
            .height(Length::Fixed(40.0))
            .into()
    };

    let mut content: Column<'_, Message, Theme> = column![top_row, buttons_bar]
        .padding(6)
        .spacing(31)
        .width(Length::Fill)
        .height(Length::Fill);

    if let Some(err) = &state.error {
        content = content.push(text(err).size(12).style(|_| text_widget::Style {
            color: Some(Color::from_rgb8(240, 200, 160)),
        }));
    }

    let fg = container(mouse_area(content).on_press(Message::DragRequest))
        .width(Length::Fill)
        .height(Length::Fill);

    let bg: Element<'_, Message, Theme> = if let Some(handle) = &state.background {
        container(
            Image::<ImageHandle>::new(handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(ContentFit::Cover),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    } else {
        text("").into()
    };

    stack![bg, fg].into()
}

fn link_button(label: &'static str, message: Message) -> button::Button<'static, Message, Theme> {
    button(text(label).size(12).style(|_| text_widget::Style {
        color: Some(Color::from_rgb8(230, 200, 150)),
    }))
    .on_press(message)
    .padding(0)
    .style(move |_, _| button::Style {
        background: None,
        text_color: Color::from_rgb8(230, 200, 150),
        border: iced::Border::default(),
        shadow: iced::Shadow::default(),
    })
}
