use crate::t;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub fn refactor_menu_keyboard(msg_id: i32, locale: &str) -> InlineKeyboardMarkup {
    let btn = |text: &str, mode: &str| {
        InlineKeyboardButton::callback(text, format!("refactor:{}:{}", mode, msg_id))
    };

    InlineKeyboardMarkup::new(vec![
        vec![btn(
            &format!(
                "👔 {}",
                t!("modules.refactor.btn_official", locale = locale)
            ),
            "official",
        )],
        vec![btn(
            &format!(
                "✍️ {}",
                t!("modules.refactor.btn_spellcheck", locale = locale)
            ),
            "spellcheck",
        )],
        vec![btn(
            &format!("✨ {}", t!("modules.refactor.btn_beauty", locale = locale)),
            "beauty",
        )],
        vec![btn(
            &format!(
                "🧠 {}",
                t!("modules.refactor.btn_formulate", locale = locale)
            ),
            "formulate",
        )],
        vec![btn(
            &format!(
                "🛠 {}",
                t!("modules.refactor.btn_group_fix", locale = locale)
            ),
            "group_fix",
        )],
        vec![btn(
            &format!(
                "🧐 {}",
                t!("modules.refactor.btn_understand", locale = locale)
            ),
            "understand",
        )],
        vec![InlineKeyboardButton::callback(
            t!("common.cancel", locale = locale),
            "delete_msg:0:0",
        )],
    ])
}
