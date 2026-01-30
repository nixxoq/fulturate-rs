use crate::{
    bot::keyboards::admin::{admin_keyboard, broadcast_mode_keyboard},
    core::config::Config,
    errors::MyError,
};
use teloxide::{
    prelude::*,
    types::{ParseMode, ReplyParameters},
};

fn check_admin_access(msg: &Message, config: &Config) -> bool {
    if !msg.chat.is_private() {
        return false;
    }

    let Some(user) = msg.from.as_ref() else {
        return false;
    };
    config.is_id_in_owners(user.id.0.to_string())
}

pub async fn admin_command_handler(bot: Bot, msg: Message, config: &Config) -> Result<(), MyError> {
    if !check_admin_access(&msg, config) {
        return Ok(());
    }

    let text = "👨‍💻 <b>Fulturate Panel</b>\n\nВыберите действие:";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .reply_markup(admin_keyboard())
        .await?;

    Ok(())
}

pub async fn broadcast_command_handler(
    bot: Bot,
    msg: Message,
    config: &Config,
) -> Result<(), MyError> {
    if !check_admin_access(&msg, config) {
        return Ok(());
    }

    let Some(reply_msg) = msg.reply_to_message() else {
        bot.send_message(
            msg.chat.id,
            "⚠️ <b>Ошибка:</b> Ответьте этой командой на сообщение, которое хотите разослать.",
        )
        .parse_mode(ParseMode::Html)
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;
        return Ok(());
    };

    let admin_id = msg.from.as_ref().unwrap().id.0;
    let redis_key = format!("broadcast_setup:{}", admin_id);
    let save_data = format!("{}:{}", reply_msg.chat.id, reply_msg.id);

    config
        .get_redis_client()
        .set(&redis_key, &save_data, 300)
        .await?;

    let text = "📢 <b>Настройка рассылки</b>\n\nВыберите режим отправки:";

    bot.copy_message(msg.chat.id, reply_msg.chat.id, reply_msg.id)
        .await?;

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .reply_markup(broadcast_mode_keyboard())
        .await?;

    Ok(())
}
