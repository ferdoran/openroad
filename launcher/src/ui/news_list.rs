use iced::widget::text as text_widget;
use iced::widget::{button, column, container, text};
use iced::{Alignment, Border, Color, Length, Theme};

use crate::{Message, ui::NewsItem};

pub fn news_list<'a>(
    items: &'a [NewsItem],
    selected: usize,
    width: Length,
) -> container::Container<'a, Message, Theme> {
    const LIST_H: f32 = 100.0;
    const ROW_H: f32 = 18.0;
    const ROW_SPACING: f32 = 2.0;

    let mut rows: Vec<iced::Element<'a, Message, Theme>> = Vec::with_capacity(items.len());

    for (index, item) in items.iter().enumerate() {
        let is_selected = index == selected;
        let background = if is_selected {
            Color::from_rgb8(54, 88, 30)
        } else {
            Color::from_rgb8(28, 28, 28)
        };

        let border = if is_selected {
            Color::from_rgb8(140, 120, 90)
        } else {
            Color::from_rgb8(90, 75, 55)
        };

        let row = button(
            text(item.title.as_str())
                .size(12)
                .shaping(text_widget::Shaping::Basic)
                .style(|_| text_widget::Style {
                    color: Some(Color::from_rgb8(230, 230, 230)),
                }),
        )
        .on_press(Message::SelectNews(index))
        .padding(2)
        .width(Length::Fill)
        .height(Length::Fixed(ROW_H))
        .style(move |_, _| button::Style {
            background: Some(background.into()),
            text_color: Color::from_rgb8(230, 230, 230),
            border: Border {
                color: border,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..button::Style::default()
        });

        rows.push(row.into());
    }

    container(column(rows).spacing(ROW_SPACING).align_x(Alignment::Start))
        .padding(0)
        .width(width)
        .height(Length::Fixed(LIST_H))
        .clip(true)
        .style(|_| container::Style {
            background: Some(Color::from_rgb8(20, 20, 20).into()),
            border: Border {
                color: Color::from_rgb8(140, 120, 90),
                width: 1.0,
                radius: 0.0.into(),
            },
            ..container::Style::default()
        })
}
