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
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode, ReplyParameters},
};

pub async fn start_handler(
    bot: Bot,
    message: Message,
    config: &Config,
    arg: String,
) -> Result<(), MyError> {
    let user_opt = message.from.as_ref();
    let locale = get_chat_locale(&message.chat, config).await;

    if arg == "register" {
        let channel_url = format!(
            "https://t.me/{}",
            config.get_channel_username().replace("@", "")
        );

        let subscribe_keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
            t!("chat.subscribe_needed_btn", locale = &locale),
            channel_url.parse()?,
        )]]);

        bot.send_message(message.chat.id, t!("chat.must_subscribe", locale = &locale))
            .reply_markup(subscribe_keyboard)
            .await?;
    }

    if let Some(user) = user_opt {
        let user_tg_lang = normalize_lang_code(user.language_code.as_deref());

        if message.chat.is_private()
            && User::find_one(doc! { "user_id": &user.id.to_string() })
                .await?
                .is_none()
        {
            User::new().user_id(user.id.to_string()).save().await?;

            let owner = Owner {
                id: user.id.to_string(),
                r#type: "user".to_string(),
            };

            Settings::create_with_defaults(&owner, user_tg_lang).await?;
        }
    }

    // if arg == "register" {
    //     return Ok(());
    // }

    let settings_data = format!(
        "settings_main:user:{}:{}",
        message.chat.id,
        message.from.as_ref().map_or(0, |u| u.id.0)
    );

    let settings_button = InlineKeyboardButton::callback(
        t!("start.btn_settings_shortcut", locale = &locale),
        settings_data,
    );

    let terms_link_button = InlineKeyboardButton::url(
        t!("start.btn_terms", locale = &locale),
        "https://fulturate.bot/terms".parse()?,
    );

    let news_link_button = InlineKeyboardButton::url(
        t!("start.btn_news", locale = &locale),
        "https://t.me/fulturate".parse()?,
    );

    bot.send_message(message.chat.id, t!("start.welcome", locale = &locale))
        .reply_parameters(ReplyParameters::new(message.id))
        .parse_mode(ParseMode::Html)
        .reply_markup(InlineKeyboardMarkup::new(vec![
            vec![news_link_button],
            vec![terms_link_button],
            vec![settings_button],
        ]))
        .await?;

    Ok(())
}
