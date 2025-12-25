use crate::{
    bot::modules::Owner,
    core::{
        config::Config,
        db::schemas::{group::Group, settings::Settings, user::User},
    },
    errors::MyError,
    util::i18n::get_locale_by_id,
    t,
};
use log::info;
use mongodb::bson::doc;
use oximod::ModelTrait;
use teloxide::{
    Bot,
    payloads::SendMessageSetters,
    prelude::Requester,
    types::{ChatMemberUpdated, InlineKeyboardButton, InlineKeyboardMarkup, ParseMode},
};

pub async fn handle_bot_added(bot: Bot, update: ChatMemberUpdated) -> Result<(), MyError> {
    let id = update.chat.id.to_string();
    let config = Config::new().await;
    let locale = get_locale_by_id(update.from.id.0, &config).await;

    if update.new_chat_member.is_banned() || update.new_chat_member.is_left() {
        info!("Bot was kicked/banned. Deleting all data for ID: {}", &id);

        let owner_type = if update.chat.is_private() {
            "user"
        } else {
            "group"
        };

        if owner_type == "user" {
            User::delete(doc! { "user_id": &id }).await.ok();
        } else {
            Group::delete(doc! { "group_id": &id }).await.ok();
        }

        Settings::delete(doc! { "owner_id": &id, "owner_type": owner_type })
            .await
            .ok();

        return Ok(());
    }

    info!("Bot added to chat. ID: {}", &id);

    let welcome_text = if update.chat.is_private() {
        t!("chat.bot_added_private", locale = &locale)
    } else {
        t!("chat.bot_added_group", locale = &locale)
    };

    let news_link_button = InlineKeyboardButton::url(
        t!("start.btn_news", locale = &locale),
        "https://t.me/fulturate".parse()?,
    );
    let terms_of_use_link_button = InlineKeyboardButton::url(
        t!("start.btn_terms", locale = &locale),
        "https://telegra.ph/Terms-Of-Use--Usloviya-ispolzovaniya-09-21".parse()?,
    );

    bot.send_message(update.chat.id, welcome_text)
        .parse_mode(ParseMode::Html)
        .reply_markup(InlineKeyboardMarkup::new(vec![vec![
            news_link_button,
            terms_of_use_link_button,
        ]]))
        .await?;

    if update.chat.is_private() {
        if User::find_one(doc! { "user_id": &id }).await?.is_none() {
            User::new().user_id(id.clone()).save().await?;
        }
        let owner = Owner {
            id,
            r#type: "user".to_string(),
        };
        Settings::get_or_create(&owner).await?;
    } else {
        if Group::find_one(doc! { "group_id": &id }).await?.is_none() {
            Group::new().group_id(id.clone()).save().await?;
        }
        let owner = Owner {
            id,
            r#type: "group".to_string(),
        };
        Settings::get_or_create(&owner).await?;
    }

    Ok(())
}
