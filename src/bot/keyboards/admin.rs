use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub fn admin_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            "📊 System Health",
            "admin:health",
        )],
        vec![InlineKeyboardButton::callback(
            "📢 Broadcast Info",
            "admin:broadcast_help",
        )],
        vec![
            InlineKeyboardButton::callback("🔄 Refresh", "admin:refresh"),
            InlineKeyboardButton::callback("❌ Close", "delete_msg:0:0"),
        ],
    ])
}

pub fn broadcast_mode_keyboard() -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            "👤 С автором (Forward)",
            "admin:broadcast:mode:forward",
        )],
        vec![InlineKeyboardButton::callback(
            "🕵️ Без автора (Copy)",
            "admin:broadcast:mode:copy",
        )],
        vec![InlineKeyboardButton::callback(
            "❌ Отмена",
            "admin:broadcast:cancel",
        )],
    ])
}

pub fn confirm_broadcast_keyboard(users_count: u64, mode: &str) -> InlineKeyboardMarkup {
    let mode_text = if mode == "forward" {
        "С автором"
    } else {
        "Без автора"
    };

    InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            format!("✅ Отправить ({}) | {}", users_count, mode_text),
            "admin:broadcast:confirm",
        )],
        vec![InlineKeyboardButton::callback(
            "❌ Отмена",
            "admin:broadcast:cancel",
        )],
    ])
}
