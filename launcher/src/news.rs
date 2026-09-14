use crate::ui::NewsItem;
use crate::{MAX_NEWS, NewsIndexItem, utils};
use iced::widget::markdown;

fn build_news_item(title: &str, preview: &str) -> NewsItem {
    NewsItem {
        title: title.to_string(),
        preview: preview.to_string(),
        markdown: markdown::parse(preview).collect(),
    }
}

pub fn default() -> Vec<NewsItem> {
    vec![build_news_item(
        "Welcome to OpenRoad",
        "News feed not configured. Set NEWS_JSON_URL to load updates.",
    )]
}

pub(crate) async fn load_from_env() -> Result<Vec<NewsItem>, String> {
    let base = utils::get_env_url("NEWS_JSON_URL")
        .ok_or_else(|| "NEWS_JSON_URL is not set. News feed will not work.".to_string())?;
    load_news_from_url(&base)
}

fn load_news_from_url(base: &str) -> Result<Vec<NewsItem>, String> {
    let base = utils::normalize_base_url(base);
    let json_url = format!("{base}news.json");
    let json = utils::read_text_resource(&json_url)?;
    let mut items: Vec<NewsIndexItem> =
        serde_json::from_str(&json).map_err(|e| format!("news.json parse failed: {e}"))?;

    items.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    let mut news = Vec::new();
    for item in items.into_iter().take(MAX_NEWS) {
        let md_path = if utils::is_http_url(&item.md) {
            item.md
        } else {
            format!("{base}{}", item.md)
        };
        let preview = utils::read_text_resource(&md_path).unwrap_or_default();
        news.push(build_news_item(&item.title, &preview));
    }

    if news.is_empty() {
        Err("NEWS_JSON_PATH returned no items.".to_string())
    } else {
        Ok(news)
    }
}
