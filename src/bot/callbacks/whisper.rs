use crate::{
    bot::inlines::whisper::Whisper, core::config::Config, errors::MyError, t,
    util::i18n::get_locale_by_id,
};
use teloxide::{
    Bot,
    payloads::{AnswerCallbackQuerySetters, EditMessageTextSetters},
    prelude::{CallbackQuery, Requester},
    types::InlineKeyboardMarkup,
};

// TODO: refactor entire handler
pub async fn handle_whisper_callback(
    bot: Bot,
    q: CallbackQuery,
    config: &Config,
) -> Result<(), MyError> {
    let data = q.data.as_ref().ok_or("Callback query data is empty")?;

    let parts: Vec<&str> = data.split('_').collect();
    if parts.len() != 3 || parts[0] != "whisper" {
        return Ok(());
    }

    let action = parts[1];
    let whisper_id = parts[2];

    let user = q.from.clone();
    let locale = get_locale_by_id(user.id.0, &config).await;

    let redis_key = format!("whisper:{}", whisper_id);

    let whisper: Option<Whisper> = config.get_redis_client().get(&redis_key).await?;

    let whisper = match whisper {
        Some(w) => w,
        None => {
            bot.answer_callback_query(q.id)
                .text(t!("whisper.expired", locale = &locale))
                .show_alert(true)
                .await?;
            return Ok(());
        }
    };

    let is_sender = user.id.0 == whisper.sender_id;
    let is_recipient = whisper.recipients.iter().any(|r| {
        if r.id == Some(user.id.0) {
            return true;
        }

        if let (Some(recipient_username), Some(username)) = (
            &r.username,
            &user.username.as_ref().map(|s| s.to_lowercase()),
        ) && recipient_username == username
        {
            return true;
        }
        false
    });

    if !is_sender && !is_recipient {
        bot.answer_callback_query(q.id)
            .text(t!("modules.whisper.not_for_you", locale = &locale))
            .show_alert(true)
            .await?;
        return Ok(());
    }

    match action {
        "read" => {
            bot.answer_callback_query(q.id)
                .text(whisper.content.to_string())
                .show_alert(true)
                .await?;
        }
        "forget" => {
            config.get_redis_client().delete(&redis_key).await?;
            bot.answer_callback_query(q.id)
                .text(t!("whisper.forgotten_alert", locale = &locale))
                .await?;

            if let Some(message) = q.message {
                let text = t!(
                    "whisper.forgotten_msg",
                    locale = &locale,
                    name = whisper.sender_first_name
                );

                bot.edit_message_text(message.chat().id, message.id(), text)
                    .reply_markup(InlineKeyboardMarkup::new(vec![vec![]]))
                    .await?;
            }
        }
        _ => {}
    }

    Ok(())
}
