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

pub fn confirm_broadcast_keyboard(users_count: u64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(
            format!("✅ Отправить ({})", users_count),
            "admin:broadcast:confirm",
        )],
        vec![InlineKeyboardButton::callback(
            "❌ Отмена",
            "admin:broadcast:cancel",
        )],
    ])
}
