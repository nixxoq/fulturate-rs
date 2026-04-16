use crate::{
    t,
    util::paginator::{FrameBuild, Paginator},
};
// use rust_i18n::t;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub const TRANSCRIPTION_MODULE_KEY: &str = "speech";
pub const SUMMARY_MODULE_KEY: &str = "summary";

pub fn create_transcription_keyboard(
    current_page: usize,
    total_pages: usize,
    user_id: u64,
    locale: &str,
) -> InlineKeyboardMarkup {
    let summary_button = InlineKeyboardButton::callback(
        t!("speech.btn_summarize", locale = locale),
        "summarize".to_string(),
    );
    let retell_button = InlineKeyboardButton::callback(
        t!("speech.btn_retell", locale = locale),
        "retell".to_string(),
    );

    let delete_button = InlineKeyboardButton::callback(
        t!("common.delete", locale = locale),
        format!("delete_msg:{}", user_id),
    );

    Paginator::new(TRANSCRIPTION_MODULE_KEY, total_pages)
        .current_page(current_page)
        .add_bottom_row(vec![summary_button, retell_button])
        .add_bottom_row(vec![delete_button])
        .build()
}

pub fn create_summary_keyboard(locale: &str) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        t!("common.back", locale = locale),
        "back_to_full",
    )]])
}

pub fn create_summary_pagination_keyboard(
    current_page: usize,
    total_pages: usize,
    locale: &str,
) -> InlineKeyboardMarkup {
    let back_button =
        InlineKeyboardButton::callback(t!("common.back", locale = locale), "back_to_full");

    Paginator::new(SUMMARY_MODULE_KEY, total_pages)
        .current_page(current_page)
        .add_bottom_row(vec![back_button])
        .build()
}

pub fn create_retry_keyboard(
    message_id: i32,
    action_type: &str,
    attempt: u32,
    locale: &str,
) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        t!("speech.retry_btn", locale = locale),
        format!("retry_speech:{}:{}:{}", message_id, action_type, attempt),
    )]])
}
