use crate::{
    bot::{
        keyboards::refactor::refactor_menu_keyboard,
        modules::{Owner, refactor::RefactorSettings},
    },
    core::{
        config::Config,
        db::schemas::settings::Settings,
        services::refactor::{RefactorMode, process_text},
    },
    errors::MyError,
    t,
    util::{html::message_to_html, i18n::get_chat_locale},
};
use teloxide::{
    prelude::*,
    types::{ParseMode, ReplyParameters},
    utils::html::escape,
};

pub async fn refactor_command_handler(
    bot: Bot,
    msg: Message,
    config: &Config,
) -> Result<(), MyError> {
    let locale = get_chat_locale(&msg.chat, config).await;

    let Some(reply) = msg.reply_to_message() else {
        bot.send_message(msg.chat.id, t!("refactor.reply_hint", locale = &locale))
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
        return Ok(());
    };

    let text = match reply.text().or(reply.caption()) {
        Some(t) => t,
        None => {
            bot.send_message(msg.chat.id, t!("errors.reply_to_text", locale = &locale))
                .await?;
            return Ok(());
        }
    };

    let owner = Owner {
        id: msg.chat.id.to_string(),
        r#type: if msg.chat.is_private() {
            "user".to_string()
        } else {
            "group".to_string()
        },
    };
    let settings: RefactorSettings = Settings::get_module_settings(&owner, "refactor").await?;

    if !settings.enabled {
        bot.send_message(msg.chat.id, t!("errors.module_disabled", locale = &locale))
            .await?;
        return Ok(());
    }

    let redis = config.get_redis_client();
    let cache_key = format!("refactor_src:{}", reply.id.0);
    redis.set(&cache_key, &text.to_string(), 3600).await?;

    bot.send_message(msg.chat.id, t!("refactor.menu_title", locale = &locale))
        .reply_parameters(ReplyParameters::new(reply.id))
        .reply_markup(refactor_menu_keyboard(reply.id.0, &locale))
        .await?;

    Ok(())
}

pub async fn understand_command_handler(
    bot: Bot,
    msg: Message,
    config: &Config,
) -> Result<(), MyError> {
    let locale = get_chat_locale(&msg.chat, config).await;

    let Some(reply) = msg.reply_to_message() else {
        bot.send_message(msg.chat.id, t!("refactor.reply_hint", locale = &locale))
            .await?;
        return Ok(());
    };

    let text = if let Some(entities) = reply.entities() {
        message_to_html(reply.text().unwrap_or_default(), entities)
    } else if let Some(entities) = reply.caption_entities() {
        message_to_html(reply.caption().unwrap_or_default(), entities)
    } else {
        escape(reply.text().or(reply.caption()).unwrap_or_default())
    };

    if text.is_empty() {
        bot.send_message(msg.chat.id, t!("errors.reply_to_text", locale = &locale))
            .await?;
        return Ok(());
    }

    let processing_msg = bot
        .send_message(msg.chat.id, t!("common.processing", locale = &locale))
        .reply_parameters(ReplyParameters::new(reply.id))
        .await?;

    match process_text(config, &text, RefactorMode::Formulate).await {
        Ok(result) => {
            if (bot
                .edit_message_text(msg.chat.id, processing_msg.id, &result)
                .parse_mode(ParseMode::Html)
                .await)
                .is_err()
            {
                bot.edit_message_text(msg.chat.id, processing_msg.id, result)
                    .await?;
            }
        }
        Err(e) => {
            log::error!("Refactor error: {:?}", e);
            bot.edit_message_text(msg.chat.id, processing_msg.id, format!("❌ Error: {}", e))
                .await?;
        }
    }

    Ok(())
}
