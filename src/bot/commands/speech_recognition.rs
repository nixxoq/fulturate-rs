use crate::{
    core::{config::Config, services::speech_recognition::transcription_handler},
    errors::MyError,
    t,
    util::i18n::get_chat_locale,
};
use teloxide::{prelude::*, types::ReplyParameters};

pub async fn speech_recognition_handler(
    bot: Bot,
    msg: Message,
    config: &Config,
) -> Result<(), MyError> {
    let locale = get_chat_locale(&msg.chat, config).await;

    let Some(message) = msg.reply_to_message() else {
        bot.send_message(msg.chat.id, t!("errors.reply_to_voice", locale = &locale))
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;

        return Ok(());
    };

    transcription_handler(bot, message, config).await?;

    Ok(())
}
