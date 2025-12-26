use crate::{
    bot::modules::Owner,
    core::{
        config::Config,
        db::schemas::{settings::Settings, user::User},
    },
    errors::MyError,
    t,
    util::i18n::{get_chat_locale, normalize_lang_code},
};
use mongodb::bson::doc;
use oximod::Model;
use std::time::Instant;
use sysinfo::System;
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode, ReplyParameters},
};

pub async fn start_handler(
    bot: Bot,
    message: Message,
    config: &Config,
    _arg: String,
) -> Result<(), MyError> {
    let user_opt = message.from.as_ref();
    let mut is_new_user = false;

    if let Some(user) = user_opt {
        let user_tg_lang = normalize_lang_code(user.language_code.as_deref());

        if message.chat.is_private()
            && User::find_one(doc! { "user_id": &user.id.to_string() })
                .await?
                .is_none()
        {
            is_new_user = true;
            User::new().user_id(user.id.to_string()).save().await?;

            let owner = Owner {
                id: user.id.to_string(),
                r#type: "user".to_string(),
            };

            println!(
                "Creating default settings for new user {} (lang: {})",
                user.id, &user_tg_lang
            );

            Settings::create_with_defaults(&owner, user_tg_lang).await?;
        }
    }

    let locale = get_chat_locale(&message.chat, config).await;

    println!("user locale: {}", &locale);

    let version = config.get_version();

    let start_time = Instant::now();
    bot.get_me().await?;
    let api_ping = start_time.elapsed().as_millis();

    let mut system_info = System::new_all();
    system_info.refresh_all();

    let total_ram_mb = system_info.total_memory() / (1024 * 1024);
    let used_ram_mb = system_info.used_memory() / (1024 * 1024);
    let cpu_usage_percent = system_info.global_cpu_usage();

    let welcome_key = if is_new_user {
        "start.welcome_new"
    } else {
        "start.welcome_back"
    };
    let welcome_text = t!(welcome_key, locale = &locale);

    let status_text = t!(
        "start.status",
        locale = &locale,
        version = version,
        ping = api_ping,
        cpu = format!("{:.2}", cpu_usage_percent),
        ram_used = used_ram_mb,
        ram_total = total_ram_mb
    );

    let response_message = format!("{}{}", welcome_text, status_text);

    let news_link_button = InlineKeyboardButton::url(
        t!("start.btn_news", locale = &locale),
        "https://t.me/fulturate".parse()?,
    );
    let terms_of_use_link_button = InlineKeyboardButton::url(
        t!("start.btn_terms", locale = &locale),
        "https://telegra.ph/Terms-Of-Use--Usloviya-ispolzovaniya-09-21".parse()?,
    );

    bot.send_message(message.chat.id, response_message)
        .reply_parameters(ReplyParameters::new(message.id))
        .parse_mode(ParseMode::Html)
        .reply_markup(InlineKeyboardMarkup::new(vec![vec![
            news_link_button,
            terms_of_use_link_button,
        ]]))
        .await?;

    Ok(())
}
