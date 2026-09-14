pub(crate) mod background;
pub mod header;
pub mod launcher_buttons;
pub mod news_list;
pub mod news_preview;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct NewsItem {
    pub title: String,
    pub preview: String,
    pub markdown: Vec<iced::widget::markdown::Item>,
}
