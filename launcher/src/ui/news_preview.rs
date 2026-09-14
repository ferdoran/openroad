use iced::widget::text as text_widget;
use iced::widget::{column, container, scrollable, text};
use iced::{Color, Length, Theme};

use crate::{Message, ui::NewsItem};

pub fn news_preview(
    item: &'_ NewsItem,
    width: Length,
    height: Length,
) -> container::Container<'_, Message, Theme> {
    let preview_text = text(item.preview.as_str())
        .size(18)
        .width(Length::Fill)
        .style(|_| text_widget::Style {
            color: Some(Color::from_rgb8(236, 232, 220)),
        });

    let markdown_box = container(preview_text)
        .padding(iced::Padding {
            top: 4.0,
            right: 16.0,
            bottom: 8.0,
            left: 4.0,
        })
        .width(Length::Fill);

    let content = column![markdown_box].spacing(10).width(Length::Fill);

    container(scrollable(content).height(Length::Fill).width(Length::Fill))
        .padding(iced::Padding {
            top: 26.0,
            right: 8.0,
            bottom: 10.0,
            left: 10.0,
        })
        .width(width)
        .height(height)
        .clip(true)
        .style(|_| container::Style {
            ..container::Style::default()
        })
}
