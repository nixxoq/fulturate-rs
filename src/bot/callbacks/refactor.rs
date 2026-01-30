use crate::{
    core::{
        config::Config,
        services::refactor::{RefactorMode, process_text},
    },
    errors::MyError,
    t,
    util::i18n::get_chat_locale,
};
use teloxide::{prelude::*, types::ParseMode};

pub async fn handle_refactor_callback(
    bot: Bot,
    q: CallbackQuery,
    config: &Config,
    mode_str: &str,
    src_msg_id: i32,
) -> Result<(), MyError> {
    let Some(msg) = q.message.as_ref().and_then(|m| m.regular_message()) else {
        return Ok(());
    };
    let locale = get_chat_locale(&msg.chat, config).await;

    let cache_key = format!("refactor_src:{}", src_msg_id);
    let text: Option<String> = config.get_redis_client().get(&cache_key).await?;

    let Some(text) = text else {
        bot.answer_callback_query(q.id)
            .text(t!("errors.cache_expired", locale = &locale))
            .await?;
        bot.edit_message_text(
            msg.chat.id,
            msg.id,
            t!("errors.cache_expired", locale = &locale),
        )
        .await?;
        return Ok(());
    };

    bot.answer_callback_query(q.id).text("⏳ ...").await?;
    bot.edit_message_text(
        msg.chat.id,
        msg.id,
        t!("common.processing", locale = &locale),
    )
    .await?;

    let mode = RefactorMode::from_string(mode_str).unwrap_or(RefactorMode::Official);

    match process_text(config, &text, mode).await {
        Ok(result) => {
            bot.edit_message_text(msg.chat.id, msg.id, result)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        Err(e) => {
            bot.edit_message_text(msg.chat.id, msg.id, format!("Error: {}", e))
                .await?;
        }
    }

    Ok(())
}
