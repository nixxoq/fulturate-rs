use crate::{
    bot::{
        keyboards::delete::delete_message_button, messages::sounder::sound_handlers, modules::Owner,
    },
    core::config::Config,
    errors::MyError,
    util::i18n::get_chat_locale,
};
use log::error;
use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{Message, ParseMode, ReplyParameters},
    utils::html::escape,
};
use tokio::task;

pub async fn handle_speech(bot: Bot, message: Message) -> Result<(), MyError> {
    let config = Config::new().await;
    let user = message.from.clone().unwrap();

    task::spawn(async move {
        if message.forward_from_user().is_some_and(|orig| orig.is_bot) || user.is_bot {
            return;
        }

        if let Err(e) = sound_handlers(bot, message.clone(), &config).await {
            error!("Sound handler failed: {:?}", e);
        }
    });

    Ok(())
}

pub async fn handle_currency(bot: Bot, message: Message) -> Result<(), MyError> {
    let config = Arc::new(Config::new().await);

    task::spawn(async move {
        let Some(user) = message.from.clone() else {
            return;
        };
        if message.forward_from_user().is_some_and(|orig| orig.is_bot)
            || user.is_bot
            || message.via_bot.is_some()
        {
            return;
        }

        let locale = get_chat_locale(&message.chat, &config).await;
        let converter = config.get_currency_converter();
        if let Some(text) = message.text() {
            let owner = Owner {
                id: message.chat.id.to_string(),
                r#type: (if message.chat.is_private() {
                    "user"
                } else {
                    "group"
                })
                .to_string(),
            };

            match converter.process_text(text, &owner, &locale).await {
                Ok(mut results) if !results.is_empty() => {
                    results.truncate(5);

                    let formatted_blocks: Vec<String> = results
                        .into_iter()
                        .map(|result_block| {
                            format!(
                                "<blockquote expandable>{}</blockquote>",
                                escape(&result_block)
                            )
                        })
                        .collect();

                    if let Err(e) = bot
                        .send_message(message.chat.id, formatted_blocks.join("\n"))
                        .parse_mode(ParseMode::Html)
                        .reply_markup(delete_message_button(user.id.0))
                        .reply_parameters(ReplyParameters::new(message.id))
                        .await
                    {
                        error!("Failed to send currency conversion result: {:?}", e);
                    }
                }
                Err(e) => {
                    error!("Currency conversion processing error: {:?}", e);
                }
                _ => {}
            }
        }
    });

    Ok(())
}
