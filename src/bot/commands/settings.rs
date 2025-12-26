use crate::util::i18n::get_available_locales;
use crate::{
    bot::modules::{Owner, registry::MOD_MANAGER},
    core::{config::Config, db::schemas::settings::Settings},
    errors::{BotError, MyError},
    t,
    util::i18n::get_chat_locale,
};
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

pub async fn settings_command_handler(
    bot: Bot,
    message: Message,
    config: &Config,
) -> Result<(), MyError> {
    let commander = message.from.as_ref().ok_or(BotError::UserNotFound)?;
    let commander_id = commander.id.0;

    let owner_id = message.chat.id.to_string();
    let owner_type = if message.chat.is_private() {
        "user"
    } else {
        "group"
    }
    .to_string();

    let locale = get_chat_locale(&message.chat, config).await;

    let (text, keyboard) = get_main_settings_menu(&locale, &owner_type, &owner_id, commander_id);

    bot.send_message(message.chat.id, text)
        .parse_mode(teloxide::types::ParseMode::Html)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

pub fn get_main_settings_menu(
    locale: &str,
    owner_type: &str,
    owner_id: &str,
    commander_id: u64,
) -> (String, InlineKeyboardMarkup) {
    let text = t!("settings.main_header", locale = locale);

    let buttons = vec![
        vec![InlineKeyboardButton::callback(
            t!("settings.btn_language", locale = locale),
            format!("settings_lang:{}:{}:{}", owner_type, owner_id, commander_id),
        )],
        vec![InlineKeyboardButton::callback(
            t!("settings.btn_modules", locale = locale),
            format!(
                "settings_modules:{}:{}:{}",
                owner_type, owner_id, commander_id
            ),
        )],
        vec![InlineKeyboardButton::callback(
            t!("settings.btn_delete_data", locale = locale),
            format!("delete_data:{}", commander_id),
        )],
    ];

    (text.parse().unwrap(), InlineKeyboardMarkup::new(buttons))
}

pub fn get_lang_settings_menu(
    locale: &str,
    owner_type: &str,
    owner_id: &str,
    commander_id: u64,
) -> (String, InlineKeyboardMarkup) {
    let text = t!("settings.lang_header", locale = locale);

    let available = get_available_locales();
    let mut lang_buttons = Vec::new();

    for lang_code in available {
        let label = t!("meta.lang", locale = &lang_code);

        let display_label = if label == "meta.lang" {
            lang_code.to_uppercase()
        } else {
            label.to_string()
        };

        lang_buttons.push(InlineKeyboardButton::callback(
            display_label,
            format!(
                "lang_set:{}:{}:{}:{}",
                lang_code, owner_type, owner_id, commander_id
            ),
        ));
    }

    let mut rows: Vec<Vec<InlineKeyboardButton>> =
        lang_buttons.chunks(2).map(|chunk| chunk.to_vec()).collect();

    rows.push(vec![InlineKeyboardButton::callback(
        t!("common.back", locale = locale),
        format!("settings_main:{}:{}:{}", owner_type, owner_id, commander_id),
    )]);

    (text.to_string(), InlineKeyboardMarkup::new(rows))
}

pub async fn get_modules_settings_menu(
    locale: &str,
    owner_type: &str,
    owner_id: &str,
    commander_id: u64,
) -> Result<(String, InlineKeyboardMarkup), MyError> {
    let settings_doc = Settings::get_or_create(&Owner {
        id: owner_id.to_string(),
        r#type: owner_type.to_string(),
    })
    .await?;

    let text = t!("settings.modules_header", locale = locale);

    let mut kb_buttons: Vec<Vec<InlineKeyboardButton>> = MOD_MANAGER
        .get_designed_modules(owner_type)
        .into_iter()
        .map(|module| {
            let is_enabled = if let Some(json_val) = settings_doc.modules.get(module.key()) {
                json_val
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            } else {
                match module.factory_settings() {
                    Ok(default_json) => default_json
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    Err(_) => false,
                }
            };

            let key = format!("modules.{}.name", module.key());
            let tr = t!(&key, locale = locale);

            let status_icon = if is_enabled { "✅" } else { "❌" };
            let btn_text = format!(
                "{} — {}",
                status_icon,
                if tr == key { module.name() } else { &tr }
            );

            let callback_data = format!(
                "module_select:{}:{}:{}:{}",
                owner_type,
                owner_id,
                module.key(),
                commander_id
            );

            vec![InlineKeyboardButton::callback(btn_text, callback_data)]
        })
        .collect();

    kb_buttons.push(vec![InlineKeyboardButton::callback(
        t!("common.back", locale = locale),
        format!("settings_main:{}:{}:{}", owner_type, owner_id, commander_id),
    )]);

    Ok((text.parse().unwrap(), InlineKeyboardMarkup::new(kb_buttons)))
}
