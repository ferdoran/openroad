use iced::widget::svg::Handle as SvgHandle;
use iced::widget::{Svg, column, container, row, text};
use iced::{Element, Length, Theme};

use crate::{Message, link_button};

const COFFEE_SVG: &str = r##"<svg width="16" height="16" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
<path d="M3 8h14v5a6 6 0 0 1-6 6H9a6 6 0 0 1-6-6V8Z" stroke="#EFD8B0" stroke-width="2" stroke-linejoin="round"/>
<path d="M17 9h2a3 3 0 0 1 0 6h-2" stroke="#EFD8B0" stroke-width="2" stroke-linejoin="round"/>
<path d="M6 3h2M10 3h2M14 3h2" stroke="#EFD8B0" stroke-width="2" stroke-linecap="round"/>
</svg>"##;

pub fn header_links() -> Element<'static, Message, Theme> {
    // Label is generic because the destination is configurable (SITE_URL); a
    // hardcoded domain here would contradict whatever the operator configured.
    let site = link_button("Website", Message::OpenSite);

    let coffee_icon = Svg::new(SvgHandle::from_memory(COFFEE_SVG.as_bytes().to_vec()))
        .width(Length::Fixed(14.0))
        .height(Length::Fixed(14.0));

    let coffee = row![
        coffee_icon,
        link_button("Buy me a coffee", Message::OpenCoffee),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center);

    let right = column![site, coffee]
        .spacing(4)
        .align_x(iced::Alignment::End);

    row![container(text("")).width(Length::Fill), right]
        .width(Length::Fill)
        .into()
}
