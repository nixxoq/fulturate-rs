use crate::{
    bot::{keyboards::delete::confirm_delete_keyboard, modules::Owner},
    core::{
        config::Config,
        db::schemas::{
            group::Group as GroupSchema, settings::Settings as SettingsSchema,
            user::User as UserSchema,
        },
        services::speech_recognition::back_handler,
    },
    errors::MyError,
    util::is_admin_or_author,
};
use log::error;
use mongodb::bson::doc;
use oximod::Model;
use redis_macros::{FromRedisValue, ToRedisArgs};
use serde::{Deserialize, Serialize};
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, MessageEntity, User},
};

#[derive(Serialize, Deserialize, FromRedisValue, ToRedisArgs)]
struct RevertState {
    text: String,
    entities: Option<Vec<MessageEntity>>,
    keyboard: Option<InlineKeyboardMarkup>,
}

async fn has_data_delete_permission(
    bot: &Bot,
    chat: &teloxide::types::Chat,
    clicker: &User,
) -> bool {
    if chat.is_private() {
        return true;
    }
    if (chat.is_group() || chat.is_supergroup())
        && let Ok(member) = bot.get_chat_member(chat.id, clicker.id).await
    {
        return member.is_owner();
    }
    false
}

pub async fn handle_delete_data(bot: Bot, query: CallbackQuery) -> Result<(), MyError> {
    let Some(message) = query.message.as_ref() else {
        return Ok(());
    };

    let can_delete = has_data_delete_permission(&bot, message.chat(), &query.from).await;

    if !can_delete {
        bot.answer_callback_query(query.id)
            .text("❌ Удалить данные чата может только его владелец.")
            .show_alert(true)
            .await?;
        return Ok(());
    }

    let (owner_type, owner_id, confirmation_text) = if message.chat().is_private() {
        (
            "user",
            query.from.id.to_string(),
            "Вы уверены, что хотите удалить все <b>ваши</b> данные из бота?\n\n<b>Это действие необратимо!</b>",
        )
    } else {
        (
            "group",
            message.chat().id.to_string(),
            "Вы уверены, что хотите удалить все данные этого <b>чата</b> из бота?\n\n<b>Это действие необратимо!</b>",
        )
    };

    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(
            "Да, удалить",
            format!("delete_data_confirm:{}:{}:yes", owner_type, owner_id),
        ),
        InlineKeyboardButton::callback(
            "Нет, отмена",
            format!("delete_data_confirm:{}:{}:no", owner_type, owner_id),
        ),
    ]]);

    bot.answer_callback_query(query.id).await?;
    bot.edit_message_text(message.chat().id, message.id(), confirmation_text)
        .reply_markup(keyboard)
        .parse_mode(teloxide::types::ParseMode::Html)
        .await?;

    Ok(())
}

pub async fn handle_delete_request(
    bot: Bot,
    query: CallbackQuery,
    config: &Config,
) -> Result<(), MyError> {
    let Some(message) = query.message.as_ref().and_then(|m| m.regular_message()) else {
        return Ok(());
    };
    let Some(data) = query.data.as_ref() else {
        return Ok(());
    };

    let payload = data.strip_prefix("delete_msg:").unwrap_or_default();
    let parts: Vec<&str> = payload.split(':').collect();

    if parts.is_empty() || parts.len() > 2 {
        return Ok(());
    }

    let Ok(target_user_id) = parts[0].parse::<u64>() else {
        bot.answer_callback_query(query.id)
            .text("❌ Ошибка: неверный ID в кнопке.")
            .show_alert(true)
            .await?;
        return Ok(());
    };

    let with_confirmation = parts.get(1).is_none_or(|&flag| flag == "1");

    let can_delete = is_admin_or_author(
        &bot,
        message.chat.id,
        message.chat.is_group() || message.chat.is_supergroup(),
        &query.from,
        target_user_id,
    )
    .await;

    if !can_delete {
        bot.answer_callback_query(query.id)
            .text("❌ Удалить может только автор сообщения или администратор.")
            .show_alert(true)
            .await?;
        return Ok(());
    }

    bot.answer_callback_query(query.id).await?;

    if with_confirmation {
        let cache = config.get_redis_client();
        let revert_key = format!("revert_state:{}", message.id);
        let revert_state = RevertState {
            text: message.text().unwrap_or_default().to_string(),
            entities: message.entities().map(|e| e.to_vec()),
            keyboard: message.reply_markup().cloned(),
        };
        cache.set(&revert_key, &revert_state, 600).await?;

        bot.edit_message_text(
            message.chat.id,
            message.id,
            "Вы уверены, что хотите удалить?",
        )
        .reply_markup(confirm_delete_keyboard(target_user_id))
        .await?;
    } else {
        bot.delete_message(message.chat.id, message.id)
            .await
            .map_err(|e| error!("Failed to delete message without confirmation: {:?}", e))
            .ok();
    }

    Ok(())
}

pub async fn handle_delete_confirmation(
    bot: Bot,
    query: CallbackQuery,
    config: &Config,
) -> Result<(), MyError> {
    let Some(message) = query.message.as_ref().and_then(|m| m.regular_message()) else {
        return Ok(());
    };
    let Some(data) = query.data.as_ref() else {
        return Ok(());
    };

    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() != 3 {
        return Ok(());
    };

    let Ok(target_user_id) = parts[1].parse::<u64>() else {
        return Ok(());
    };
    let action = parts[2];

    let can_delete = is_admin_or_author(
        &bot,
        message.chat.id,
        message.chat.is_group() || message.chat.is_supergroup(),
        &query.from,
        target_user_id,
    )
    .await;

    if !can_delete {
        bot.answer_callback_query(query.id)
            .text("❌ У вас нет прав для этого действия.")
            .show_alert(true)
            .await?;
        return Ok(());
    }

    bot.answer_callback_query(query.clone().id).await?;

    match action {
        "yes" => {
            bot.delete_message(message.chat.id, message.id)
                .await
                .map_err(|e| error!("Failed to delete message: {:?}", e))
                .ok();
        }
        "no" => {
            let cache = config.get_redis_client();
            let revert_key = format!("revert_state:{}", message.id);
            let revert_state: Option<RevertState> = cache.get_and_delete(&revert_key).await?;

            if let Some(state) = revert_state {
                let mut edit_request =
                    bot.edit_message_text(message.chat.id, message.id, state.text);
                if let Some(keyboard) = state.keyboard {
                    edit_request = edit_request.reply_markup(keyboard);
                }
                if let Some(entities) = state.entities {
                    edit_request = edit_request.entities(entities);
                }
                edit_request.await?;
            } else {
                let message_cache_key = format!("message_file_map:{}", message.id);
                let is_transcription_message = cache
                    .get::<String>(&message_cache_key)
                    .await
                    .is_ok_and(|v| v.is_some());

                if is_transcription_message {
                    back_handler(bot, query, config).await?;
                } else {
                    bot.edit_message_text(message.chat.id, message.id, "✅ Действие отменено.")
                        .reply_markup(InlineKeyboardMarkup::new(vec![vec![]]))
                        .await?;
                }
            }
        }
        _ => {}
    }

    Ok(())
}

pub async fn handle_delete_data_confirmation(
    bot: Bot,
    query: CallbackQuery,
) -> Result<(), MyError> {
    let Some(message) = query.message.as_ref() else {
        return Ok(());
    };
    let Some(data) = query.data.as_ref() else {
        return Ok(());
    };

    let parts: Vec<&str> = data
        .strip_prefix("delete_data_confirm:")
        .unwrap_or_default()
        .split(':')
        .collect();
    if parts.len() != 3 {
        return Ok(());
    }

    let owner_type = parts[0];
    let owner_id = parts[1];
    let action = parts[2];

    let can_delete = has_data_delete_permission(&bot, message.chat(), &query.from).await;
    if !can_delete {
        bot.answer_callback_query(query.id)
            .text("❌ У вас нет прав для этого действия.")
            .show_alert(true)
            .await?;
        return Ok(());
    }

    bot.answer_callback_query(query.id).await?;

    match action {
        "yes" => {
            let owner = Owner {
                id: owner_id.to_string(),
                r#type: owner_type.to_string(),
            };

            SettingsSchema::delete_one(doc! { "owner_id": &owner.id, "owner_type": &owner.r#type })
                .await?;

            if owner.r#type == "user" {
                UserSchema::delete_one(doc! { "user_id": &owner.id }).await?;
            } else if owner.r#type == "group" {
                GroupSchema::delete_one(doc! { "group_id": &owner.id }).await?;
            }

            let final_text = if owner.r#type == "user" {
                "✅ Все ваши данные были успешно удалены."
            } else {
                "✅ Все данные этого чата были успешно удалены."
            };

            bot.edit_message_text(message.chat().id, message.id(), final_text)
                .reply_markup(InlineKeyboardMarkup::new(vec![vec![]]))
                .await?;
        }
        "no" => {
            bot.edit_message_text(
                message.chat().id,
                message.id(),
                "✅ Удаление данных отменено.",
            )
            .reply_markup(InlineKeyboardMarkup::new(vec![vec![]]))
            .await?;
        }
        _ => {}
    }

    Ok(())
}
