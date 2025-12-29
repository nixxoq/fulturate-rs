use crate::{
    bot::keyboards::admin::admin_keyboard,
    core::{
        config::Config,
        db::schemas::{OrmFunction, user::User},
    },
    errors::MyError,
};
use chrono::Local;
use std::time::{Duration, SystemTime};
use sysinfo::{Pid, System};
use teloxide::{
    prelude::*,
    types::{MessageId, ParseMode},
};
use tokio::time::sleep;

pub async fn handle_admin_callback(
    bot: Bot,
    q: CallbackQuery,
    config: &Config,
) -> Result<(), MyError> {
    let Some(data) = q.data else {
        return Ok(());
    };
    let Some(msg) = q.message else {
        return Ok(());
    };
    let admin_id = q.from.id.0;

    if !config.is_id_in_owners(admin_id.to_string()) {
        bot.answer_callback_query(q.id)
            .text("⛔ Access Denied")
            .await?;
        return Ok(());
    }

    match data.as_str() {
        "admin:health" | "admin:refresh" => {
            let mut sys = System::new_all();
            sys.refresh_all();

            let pid = Pid::from(std::process::id() as usize);

            let (memory_used_mb, uptime_seconds) = if let Some(process) = sys.process(pid) {
                (process.memory() / 1024 / 1024, process.run_time())
            } else {
                (0, 0)
            };
            let total_ram_gb = sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0;

            let days = uptime_seconds / 86400;
            let hours = (uptime_seconds % 86400) / 3600;
            let minutes = (uptime_seconds % 3600) / 60;

            let redis_status = match config.get_redis_client().get::<String>("test_ping").await {
                Ok(_) => "🟢 Online",
                Err(_) => "🔴 Offline",
            };

            let users_count = User::query().count().await.unwrap_or(0);

            let active_users = User::query()
                .filter_op("download_count", "$gt", 0)
                .count()
                .await
                .unwrap_or(0);

            let server_time = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

            let text = format!(
                "📊 <b>System Health</b>\n\n\
                🖥 <b>System:</b>\n\
                • RAM: <code>{} MB / {:.1} GB</code>\n\
                • Uptime: <code>{}d {}h {}m</code>\n\
                • OS: <code>{}</code>\n\n\
                🗄 <b>Database:</b>\n\
                • Users Total: <code>{}</code>\n\
                • Users Active (w/ downloads): <code>{}</code>\n\
                • Redis: {}\n\n\
                🦀 <b>Bot:</b>\n\
                • Version: <code>{}</code>\n\
                • Time: <code>{}</code>",
                memory_used_mb,
                total_ram_gb,
                days,
                hours,
                minutes,
                std::env::consts::OS,
                users_count,
                active_users,
                redis_status,
                config.get_version(),
                server_time
            );

            bot.edit_message_text(msg.chat().id, msg.id(), text)
                .parse_mode(ParseMode::Html)
                .reply_markup(admin_keyboard())
                .await?;

            bot.answer_callback_query(q.id).await?;
        }

        "admin:broadcast:cancel" => {
            let redis_key = format!("broadcast_pending:{}", admin_id);
            config.get_redis_client().delete(&redis_key).await?;

            bot.edit_message_text(msg.chat().id, msg.id(), "❌ <b>Рассылка отменена.</b>")
                .parse_mode(ParseMode::Html)
                .await?;
            bot.answer_callback_query(q.id).text("Отменено").await?;
        }

        "admin:broadcast:confirm" => {
            let redis_key = format!("broadcast_pending:{}", admin_id);

            let saved_data: Option<String> = config.get_redis_client().get(&redis_key).await?;

            let Some(data_str) = saved_data else {
                bot.answer_callback_query(q.id)
                    .text("⏳ Время ожидания истекло")
                    .show_alert(true)
                    .await?;
                bot.edit_message_text(
                    msg.chat().id,
                    msg.id(),
                    "❌ <b>Данные устарели.</b> Повторите команду.",
                )
                .parse_mode(ParseMode::Html)
                .await?;
                return Ok(());
            };

            config.get_redis_client().delete(&redis_key).await?;

            let parts: Vec<&str> = data_str.split(':').collect();
            if parts.len() != 2 {
                bot.answer_callback_query(q.id)
                    .text("❌ Ошибка данных")
                    .await?;
                return Ok(());
            }
            let from_chat_id = parts[0].parse::<i64>().unwrap();
            let message_id = parts[1].parse::<i32>().unwrap();

            bot.edit_message_text(
                msg.chat().id,
                msg.id(),
                "🚀 <b>Рассылка запущена...</b>\n<i>Я пришлю отчет по завершении.</i>",
            )
            .parse_mode(ParseMode::Html)
            .await?;

            let bot_clone = bot.clone();
            let admin_chat_id = msg.chat().id;

            tokio::spawn(async move {
                let users = match User::query().all().await {
                    Ok(u) => u,
                    Err(_) => return,
                };

                let total = users.len();
                let mut success = 0;
                let mut blocked = 0;
                let mut failed = 0;
                let start_time = SystemTime::now();

                for (i, user) in users.iter().enumerate() {
                    if i > 0 && i % 25 == 0 {
                        sleep(Duration::from_secs(1)).await;
                    }

                    match bot_clone
                        .copy_message(
                            ChatId(user.user_id.parse().unwrap_or(0)),
                            ChatId(from_chat_id),
                            MessageId(message_id),
                        )
                        .await
                    {
                        Ok(_) => success += 1,
                        Err(teloxide::RequestError::Api(
                            teloxide::ApiError::BotBlocked | teloxide::ApiError::UserDeactivated,
                        )) => {
                            blocked += 1;
                            // todo: мб удалять его?
                        }
                        Err(_e) => {
                            failed += 1;
                        }
                    }
                }

                let duration = SystemTime::now()
                    .duration_since(start_time)
                    .unwrap_or_default()
                    .as_secs();

                let report = format!(
                    "✅ <b>Рассылка завершена!</b>\n\n\
                    ⏱ Время: {} сек\n\
                    👥 Всего в базе: {}\n\
                    ✅ Доставлено: {}\n\
                    🚫 Заблокировали: {}\n\
                    ❌ Ошибки: {}",
                    duration, total, success, blocked, failed
                );

                let _ = bot_clone
                    .send_message(admin_chat_id, report)
                    .parse_mode(ParseMode::Html)
                    .await;
            });
        }
        "admin:broadcast_help" => {
            bot.answer_callback_query(q.id)
                .text("Инструкция в чате")
                .await?;
            let help_text = "📢 <b>Как сделать рассылку:</b>\n\n\
                             1. Напишите пост (текст, фото, видео - что угодно).\n\
                             2. Сделайте ответ на этот пост командой <code>/broadcast</code>.\n\n\
                             Бот скопирует сообщение и разошлет всем пользователям из базы.";

            bot.send_message(msg.chat().id, help_text)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        _ => {}
    }

    Ok(())
}
