use crate::{
    bot::keyboards::admin::{admin_keyboard, confirm_broadcast_keyboard},
    core::{
        config::Config,
        db::schemas::{OrmFunction, user::User},
    },
    errors::MyError,
};
use chrono::Local;
use std::fmt::Write;
use std::time::{Duration, SystemTime};
use sysinfo::{Pid, System};
use teloxide::{
    prelude::*,
    types::{InputFile, MessageId, ParseMode},
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

    let clear_redis = |client: crate::core::db::redis::RedisCache, uid: u64| async move {
        let _ = client.delete(&format!("broadcast_setup:{}", uid)).await;
        let _ = client.delete(&format!("broadcast_pending:{}", uid)).await;
    };

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

        "admin:broadcast:mode:forward" | "admin:broadcast:mode:copy" => {
            let setup_key = format!("broadcast_setup:{}", admin_id);
            let saved_msg_data: Option<String> = config.get_redis_client().get(&setup_key).await?;

            let Some(msg_data) = saved_msg_data else {
                bot.answer_callback_query(q.id)
                    .text("⏳ Данные устарели")
                    .show_alert(true)
                    .await?;
                bot.delete_message(msg.chat().id, msg.id()).await?;
                return Ok(());
            };

            let mode = if data.contains("forward") {
                "forward"
            } else {
                "copy"
            };

            let pending_key = format!("broadcast_pending:{}", admin_id);
            let final_data = format!("{}:{}", msg_data, mode);

            config
                .get_redis_client()
                .set(&pending_key, &final_data, 300)
                .await?;

            let users_count = User::query().count().await.unwrap_or(0);

            let text = format!(
                "📢 <b>Подтверждение рассылки</b>\n\n\
                👥 Получателей: <b>{}</b>\n\
                ⚙️ Режим: <b>{}</b>\n\n\
                ⚠️ <i>Действие необратимо. Начать?</i>",
                users_count,
                if mode == "forward" {
                    "Пересылка (с автором)"
                } else {
                    "Копия (без автора)"
                }
            );

            bot.edit_message_text(msg.chat().id, msg.id(), text)
                .parse_mode(ParseMode::Html)
                .reply_markup(confirm_broadcast_keyboard(users_count, mode))
                .await?;

            bot.answer_callback_query(q.id).await?;
        }

        "admin:broadcast:cancel" => {
            clear_redis(config.get_redis_client().clone(), admin_id).await;
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
            if parts.len() != 3 {
                bot.answer_callback_query(q.id)
                    .text("❌ Ошибка данных")
                    .await?;
                return Ok(());
            }

            let from_chat_id = parts[0].parse::<i64>().unwrap();
            let message_id = parts[1].parse::<i32>().unwrap();
            let mode = parts[2].to_string();

            bot.edit_message_text(
                msg.chat().id,
                msg.id(),
                format!(
                    "🚀 <b>Рассылка запущена (Mode: {})...</b>\n<i>Отчет придет по завершении.</i>",
                    mode
                ),
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

                let mut error_log = String::new();
                writeln!(&mut error_log, "Broadcast Error Report").unwrap();
                writeln!(
                    &mut error_log,
                    "Time: {}",
                    Local::now().format("%Y-%m-%d %H:%M:%S")
                )
                .unwrap();
                writeln!(&mut error_log, "Mode: {}", mode).unwrap();
                writeln!(&mut error_log, "-------------------------\n").unwrap();

                let start_time = SystemTime::now();

                for (i, user) in users.iter().enumerate() {
                    if i > 0 && i % 25 == 0 {
                        sleep(Duration::from_secs(1)).await;
                    }

                    let chat_target = ChatId(user.user_id.parse().unwrap_or(0));

                    let result = if mode == "forward" {
                        bot_clone
                            .forward_message(
                                chat_target,
                                ChatId(from_chat_id),
                                MessageId(message_id),
                            )
                            .await
                            .map(|_| ())
                    } else {
                        bot_clone
                            .copy_message(chat_target, ChatId(from_chat_id), MessageId(message_id))
                            .await
                            .map(|_| ())
                    };

                    match result {
                        Ok(_) => success += 1,
                        Err(err) => match err {
                            teloxide::RequestError::Api(api_err) => match api_err {
                                teloxide::ApiError::BotBlocked
                                | teloxide::ApiError::UserDeactivated => {
                                    blocked += 1;
                                    writeln!(
                                        &mut error_log,
                                        "[BLOCKED/DEACTIVATED] ID: {} | Error: {:?}",
                                        user.user_id, api_err
                                    )
                                    .unwrap();
                                    // TODO: remove from db?
                                }
                                _ => {
                                    failed += 1;
                                    writeln!(
                                        &mut error_log,
                                        "[API ERROR] ID: {} | Error: {:?}",
                                        user.user_id, api_err
                                    )
                                    .unwrap();
                                }
                            },
                            _ => {
                                failed += 1;
                                writeln!(
                                    &mut error_log,
                                    "[NETWORK/OTHER] ID: {} | Error: {:?}",
                                    user.user_id, err
                                )
                                .unwrap();
                            }
                        },
                    }
                }

                let duration = SystemTime::now()
                    .duration_since(start_time)
                    .unwrap_or_default()
                    .as_secs();

                let report_text = format!(
                    "✅ <b>Рассылка завершена!</b>\n\n\
                    ⚙️ Режим: <b>{}</b>\n\
                    ⏱ Время: {} сек\n\
                    👥 База: {}\n\
                    ✅ Успешно: {}\n\
                    🚫 Заблокировали: {}\n\
                    ❌ Ошибки API: {}",
                    if mode == "forward" { "Forward" } else { "Copy" },
                    duration,
                    total,
                    success,
                    blocked,
                    failed
                );

                let _ = bot_clone
                    .send_message(admin_chat_id, &report_text)
                    .parse_mode(ParseMode::Html)
                    .await;

                if blocked > 0 || failed > 0 {
                    let doc = InputFile::memory(error_log.into_bytes()).file_name(format!(
                        "broadcast_errors_{}.txt",
                        Local::now().format("%d_%m_%H_%M")
                    ));

                    let _ = bot_clone
                        .send_document(admin_chat_id, doc)
                        .caption("📄 Лог ошибок рассылки")
                        .await;
                }
            });
        }
        "admin:broadcast_help" => {
            bot.answer_callback_query(q.id).text("Инструкция").await?;
            let help_text = "📢 <b>Инструкция по рассылке:</b>\n\n\
                             1. Напишите пост в ЛС боту.\n\
                             2. Сделайте <b>Reply</b> (ответ) на этот пост командой <code>/broadcast</code>.\n\
                             3. Выберите режим:\n\
                                — <b>Forward:</b> Пользователь увидит пересланное сообщение (с ссылкой на автора/канал).\n\
                                — <b>Copy:</b> Сообщение придет от имени бота (анонимно).\n\
                             4. Подтвердите отправку.\n\n\
                             После завершения бот пришлет файл с ошибками, если они будут.";

            bot.send_message(msg.chat().id, help_text)
                .parse_mode(ParseMode::Html)
                .await?;
        }
        _ => {}
    }

    Ok(())
}
