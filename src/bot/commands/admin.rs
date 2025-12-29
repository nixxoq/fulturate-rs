use crate::{
    bot::keyboards::admin::{admin_keyboard, confirm_broadcast_keyboard},
    core::{
        config::Config,
        db::schemas::{OrmFunction, user::User},
    },
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

    let users_count = User::query().count().await.unwrap_or(0);

    let admin_id = msg.from.as_ref().unwrap().id.0;
    let redis_key = format!("broadcast_pending:{}", admin_id);

    let save_data = format!("{}:{}", reply_msg.chat.id, reply_msg.id);

    config
        .get_redis_client()
        .set(&redis_key, &save_data, 300)
        .await?;

    let text = format!(
        "📢 <b>Подготовка рассылки</b>\n\n\
        Вы собираетесь отправить это сообщение <b>{}</b> пользователям.\n\n\
        ⚠️ <i>Это действие нельзя будет отменить после начала.</i>\n\
        Подтвердите отправку:",
        users_count
    );

    bot.copy_message(msg.chat.id, reply_msg.chat.id, reply_msg.id)
        .await?;

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .reply_markup(confirm_broadcast_keyboard(users_count))
        .await?;

    Ok(())
}
