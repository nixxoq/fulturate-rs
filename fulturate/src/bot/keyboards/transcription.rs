use crate::util::paginator::{FrameBuild, Paginator};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub const TRANSCRIPTION_MODULE_KEY: &str = "speech";

pub fn create_transcription_keyboard(
    current_page: usize,
    total_pages: usize,
    user_id: u64,
) -> InlineKeyboardMarkup {
    let summary_button = InlineKeyboardButton::callback("✨ Итоги", "summarize".to_string());
    let retell_button = InlineKeyboardButton::callback("📝 Пересказ", "retell".to_string());

    let delete_button = InlineKeyboardButton::callback("🗑️ Удалить", format!("delete_msg:{}", user_id));

    Paginator::new(TRANSCRIPTION_MODULE_KEY, total_pages)
        .current_page(current_page)
        .add_bottom_row(vec![summary_button, retell_button])
        .add_bottom_row(vec![delete_button])
        .build()
}

pub fn create_summary_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "⬅️ Назад",
        "back_to_full",
    )]])
}

pub fn create_retry_keyboard(message_id: i32, action_type: &str, attempt: u32) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "🔁 Повторить попытку",
        format!("retry_speech:{}:{}:{}", message_id, action_type, attempt),
    )]/*, delete_message_button(user_id).inline_keyboard.first().unwrap().to_vec()*/])
}