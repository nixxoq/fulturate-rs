use crate::{
    bot::{
        keyboards::transcription::{
            create_retry_keyboard, create_summary_keyboard, create_summary_pagination_keyboard,
            create_transcription_keyboard,
        },
        modules::{Owner, speech_recognition::SpeechRecognitionSettings},
    },
    core::{
        config::Config,
        db::schemas::{settings::Settings as DbSettings, user::User},
    },
    errors::{BotError, MyError},
    t,
    util::{enums::AudioStruct, i18n::get_chat_locale, split_text},
};
use anyhow::anyhow;
use bytes::Bytes;
use gem_rs::{
    api::Models,
    client::GemSession,
    types::{FileManager, HarmBlockThreshold, Role, Settings},
};
use log::{debug, error, info};
use redis::AsyncCommands;
use redis_macros::{FromRedisValue, ToRedisArgs};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use teloxide::{
    prelude::*,
    types::{FileId, MessageId, MessageKind, ParseMode, ReplyParameters},
    utils::html,
};

const QUEUE_LIMIT_FREE: usize = 2; // todo: 15
const QUEUE_LIMIT_PREMIUM: usize = 50;
const REDIS_QUEUE_FREE: &str = "sr_queue:free";
const REDIS_QUEUE_PREMIUM: &str = "sr_queue:premium";

#[derive(Serialize, Deserialize, Debug)]
pub struct SpeechJob {
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub user_id: u64,
    pub file_info: AudioStruct,
    pub settings: SpeechRecognitionSettings,
}

#[derive(Debug, Serialize, Deserialize, FromRedisValue, ToRedisArgs, Clone)]
pub struct TranscriptionCache {
    pub full_text: String,
    pub summary: Option<String>,
    pub file_id: String,
    pub mime_type: String,
    pub attempt: u32,
}

pub struct Transcription {
    pub(crate) mime_type: String,
    pub(crate) data: Bytes,
    pub(crate) config: Config,
    pub(crate) custom_model: String,
}

pub async fn run_speech_worker(bot: Bot, config: Config) {
    info!("Speech worker started");
    let mut con = match config
        .get_redis_client()
        .client
        .get_multiplexed_tokio_connection()
        .await
    {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to connect to Redis in worker: {}", e);
            return;
        }
    };

    loop {
        let job_data: Option<String> =
            match con.lpop::<_, Vec<String>>(REDIS_QUEUE_PREMIUM, None).await {
                Ok(mut items) => items.pop(),
                Err(e) => {
                    error!("Redis error popping premium queue: {}", e);
                    None
                }
            };

        let job_data = if job_data.is_some() {
            job_data
        } else {
            match con.lpop::<_, Vec<String>>(REDIS_QUEUE_FREE, None).await {
                Ok(mut items) => items.pop(),
                Err(e) => {
                    error!("Redis error popping free queue: {}", e);
                    None
                }
            }
        };

        if let Some(json) = job_data {
            match serde_json::from_str::<SpeechJob>(&json) {
                Ok(job) => {
                    let bot_clone = bot.clone();
                    let config_clone = config.clone();

                    if let Err(e) = process_speech_job(bot_clone, config_clone, job).await {
                        error!("Failed to process speech job: {}", e);
                    }
                }
                Err(e) => error!("Failed to deserialize speech job: {}", e),
            }
        } else {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

async fn process_speech_job(bot: Bot, config: Config, job: SpeechJob) -> Result<(), MyError> {
    let locale = get_chat_locale(
        &teloxide::types::Chat {
            id: job.chat_id,
            kind: teloxide::types::ChatKind::Private(teloxide::types::ChatPrivate {
                username: None,
                first_name: None,
                last_name: None,
            }),
        },
        &config,
    )
    .await;

    let processing_msg = bot
        .send_message(
            job.chat_id,
            t!("speech.processing_started", locale = &locale),
        )
        .reply_parameters(ReplyParameters::new(job.message_id))
        .await?;

    let cache = config.get_redis_client();

    let message_file_map_key = format!("message_file_map:{}", processing_msg.id);
    cache
        .set(&message_file_map_key, &job.file_info.file_unique_id, 86400)
        .await?;

    let model_key = job.settings.transcription_model.api_key().to_string();

    match get_cached(&bot, &job.file_info, &config, false, model_key, &locale).await {
        Ok(cache_entry) => {
            let text_parts = split_text(&cache_entry.full_text, 4000);
            if text_parts.is_empty() {
                bot.edit_message_text(
                    job.chat_id,
                    processing_msg.id,
                    t!("speech.error_empty", locale = &locale),
                )
                .await?;
                return Ok(());
            }

            let keyboard = create_transcription_keyboard(0, text_parts.len(), job.user_id, &locale);

            bot.edit_message_text(
                job.chat_id,
                processing_msg.id,
                format!(
                    "<blockquote expandable>{}</blockquote>",
                    html::escape(&text_parts[0])
                ),
            )
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await?;
        }
        Err(e) => {
            error!("Processing failed for chat {}: {:?}", job.chat_id, e);
            let error_text = t!("speech.error_processing", locale = &locale);
            let retry_keyboard = create_retry_keyboard(job.message_id.0, "transcribe", 0, &locale);

            bot.edit_message_text(job.chat_id, processing_msg.id, error_text)
                .reply_markup(retry_keyboard)
                .await?;
        }
    }

    Ok(())
}

pub async fn transcription_handler(
    bot: Bot,
    msg: &Message,
    config: &Config,
) -> Result<(), MyError> {
    let locale = get_chat_locale(&msg.chat, config).await;

    let owner = Owner {
        id: msg.chat.id.to_string(),
        r#type: if msg.chat.is_private() {
            "user".to_string()
        } else {
            "group".to_string()
        },
    };

    let settings: SpeechRecognitionSettings = DbSettings::get_module_settings(&owner, "speech")
        .await
        .unwrap_or_default();

    if !settings.enabled {
        return Ok(());
    }

    let is_allowed = if msg.voice().is_some() {
        settings.enable_voice
    } else if msg.video_note().is_some() {
        settings.enable_video_note
    } else if msg.audio().is_some() {
        settings.enable_audio
    } else {
        false
    };

    if !is_allowed {
        return Ok(());
    }

    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    let is_premium = User::check_premium(&user.id.0.to_string()).await;

    if let Some(file_info) = get_file_id(msg).await {
        let cache_client = config.get_redis_client();
        let file_cache_key = format!("transcription_by_file:{}", &file_info.file_unique_id);

        if let Ok(Some(cached)) = cache_client
            .get::<TranscriptionCache>(&file_cache_key)
            .await
        {
            if !cached.full_text.is_empty() {
                let text_parts = split_text(&cached.full_text, 4000);

                let sent_msg = bot
                    .send_message(
                        msg.chat.id,
                        format!(
                            "<blockquote expandable>{}</blockquote>",
                            html::escape(&text_parts[0])
                        ),
                    )
                    .reply_parameters(ReplyParameters::new(msg.id))
                    .parse_mode(ParseMode::Html)
                    .reply_markup(create_transcription_keyboard(
                        0,
                        text_parts.len(),
                        user.id.0,
                        &locale,
                    ))
                    .await?;

                let message_file_map_key = format!("message_file_map:{}", sent_msg.id);
                cache_client
                    .set(&message_file_map_key, &file_info.file_unique_id, 86400)
                    .await?;

                return Ok(());
            }
        }

        let (queue_key, limit) = if is_premium {
            (REDIS_QUEUE_PREMIUM, QUEUE_LIMIT_PREMIUM)
        } else {
            (REDIS_QUEUE_FREE, QUEUE_LIMIT_FREE)
        };

        let mut con = config
            .get_redis_client()
            .client
            .get_multiplexed_tokio_connection()
            .await?;
        let queue_len: usize = con.llen(queue_key).await?;

        if queue_len >= limit {
            bot.send_message(
                msg.chat.id,
                t!(
                    "speech.queue_full",
                    locale = &locale,
                    count = queue_len,
                    limit = limit
                ),
            )
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
            return Ok(());
        }

        let job = SpeechJob {
            chat_id: msg.chat.id,
            message_id: msg.id,
            user_id: user.id.0,
            file_info,
            settings,
        };

        let job_json = serde_json::to_string(&job)?;
        let _: () = con.rpush(queue_key, job_json).await?;

        bot.send_message(
            msg.chat.id,
            t!(
                "speech.added_to_queue",
                locale = &locale,
                pos = queue_len + 1
            ),
        )
        .reply_parameters(ReplyParameters::new(msg.id))
        .parse_mode(ParseMode::Html)
        .await?;
    } else {
        bot.send_message(msg.chat.id, t!("speech.audio_not_found", locale = &locale))
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
    }

    Ok(())
}

pub async fn pagination_handler(
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
    let locale = get_chat_locale(&message.chat, config).await;

    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() != 3 || parts[1] != "page" {
        return Ok(());
    }

    let mode = parts[0];
    if mode != "speech" && mode != "summary" {
        return Ok(());
    }

    let Ok(page) = parts[2].parse::<usize>() else {
        return Ok(());
    };

    let cache = config.get_redis_client();
    let message_cache_key = format!("message_file_map:{}", message.id);
    let Some(file_unique_id): Option<String> = cache.get::<String>(&message_cache_key).await?
    else {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            t!("errors.button_expired", locale = &locale),
        )
        .await?;
        return Ok(());
    };

    let file_cache_key = format!("transcription_by_file:{}", file_unique_id);
    let Some(cache_entry) = cache.get::<TranscriptionCache>(&file_cache_key).await? else {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            t!("errors.text_not_found_cache", locale = &locale),
        )
        .await?;
        return Ok(());
    };

    let full_text_source = if mode == "summary" {
        cache_entry
            .summary
            .unwrap_or_else(|| "Error: Summary not found.".to_string())
    } else {
        cache_entry.full_text
    };

    let text_parts = split_text(&full_text_source, 4000);
    if page >= text_parts.len() {
        return Ok(());
    }

    let formatted_text = if mode == "summary" {
        format!(
            "✨:\n<blockquote expandable>{}</blockquote>",
            html::escape(&text_parts[page])
        )
    } else {
        format!(
            "<blockquote expandable>{}</blockquote>",
            html::escape(&text_parts[page])
        )
    };

    let new_keyboard = if mode == "summary" {
        create_summary_pagination_keyboard(page, text_parts.len(), &locale)
    } else {
        create_transcription_keyboard(page, text_parts.len(), query.from.id.0, &locale)
    };

    if message.text() != Some(&formatted_text) || message.reply_markup() != Some(&new_keyboard) {
        bot.edit_message_text(message.chat.id, message.id, formatted_text)
            .parse_mode(ParseMode::Html)
            .reply_markup(new_keyboard)
            .await?;
    }

    Ok(())
}

pub async fn back_handler(bot: Bot, query: CallbackQuery, config: &Config) -> Result<(), MyError> {
    let Some(message) = query.message.and_then(|m| m.regular_message().cloned()) else {
        return Ok(());
    };
    let locale = get_chat_locale(&message.chat, config).await;

    let cache = config.get_redis_client();
    let message_cache_key = format!("message_file_map:{}", message.id);
    let Some(file_unique_id) = cache.get::<String>(&message_cache_key).await? else {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            t!("speech.audio_not_found", locale = &locale),
        )
        .await?;
        return Ok(());
    };

    let file_cache_key = format!("transcription_by_file:{}", file_unique_id);
    let Some(cache_entry): Option<TranscriptionCache> =
        cache.get::<TranscriptionCache>(&file_cache_key).await?
    else {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            t!("errors.text_not_found_cache", locale = &locale),
        )
        .await?;
        return Ok(());
    };

    let text_parts = split_text(&cache_entry.full_text, 4000);
    let keyboard = create_transcription_keyboard(0, text_parts.len(), query.from.id.0, &locale);

    bot.edit_message_text(
        message.chat.id,
        message.id,
        format!(
            "<blockquote expandable>{}</blockquote>",
            html::escape(&text_parts[0])
        ),
    )
    .parse_mode(ParseMode::Html)
    .reply_markup(keyboard)
    .await?;

    Ok(())
}

pub async fn summarization_handler(
    bot: Bot,
    query: CallbackQuery,
    config: &Config,
) -> Result<(), MyError> {
    let Some(message) = query.message.and_then(|m| m.regular_message().cloned()) else {
        return Ok(());
    };
    let locale = get_chat_locale(&message.chat, config).await;

    let owner = Owner {
        id: message.chat.id.to_string(),
        r#type: if message.chat.is_private() {
            "user".to_string()
        } else {
            "group".to_string()
        },
    };
    let settings: SpeechRecognitionSettings = DbSettings::get_module_settings(&owner, "speech")
        .await
        .unwrap_or_default();

    let action_type = query.data.as_deref().unwrap_or("summarize");

    let Some(audio_message_id) = message.reply_to_message().map(|m| m.id.0) else {
        bot.answer_callback_query(query.id)
            .text(t!("speech.audio_not_found", locale = &locale))
            .show_alert(true)
            .await?;
        return Ok(());
    };

    let cache = config.get_redis_client();
    let message_file_map_key = format!("message_file_map:{}", message.id);
    let Some(file_unique_id) = cache.get::<String>(&message_file_map_key).await? else {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            t!("errors.button_expired", locale = &locale),
        )
        .await?;
        return Ok(());
    };

    let file_cache_key = format!("transcription_by_file:{}", file_unique_id);
    let mut cache_entry = match cache.get::<TranscriptionCache>(&file_cache_key).await? {
        Some(entry) => entry,
        None => {
            bot.edit_message_text(
                message.chat.id,
                message.id,
                t!("speech.audio_not_found", locale = &locale),
            )
            .await?;
            return Ok(());
        }
    };

    if let Some(cached_summary) = cache_entry.summary {
        let text_parts = split_text(&cached_summary, 4000);

        let icon = if action_type == "retell" {
            "📝"
        } else {
            "✨"
        };
        let final_text = format!(
            "{}:\n<blockquote expandable>{}</blockquote>",
            icon,
            html::escape(&text_parts[0])
        );
        let keyboard = create_summary_pagination_keyboard(0, text_parts.len(), &locale);

        bot.edit_message_text(message.chat.id, message.id, final_text)
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await?;
        return Ok(());
    }

    if let Some(text) = message.text()
        && text.contains("[no speech]")
    {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            t!("speech.no_speech", locale = &locale),
        )
        .parse_mode(ParseMode::Html)
        .reply_markup(create_summary_keyboard(&locale))
        .await?;
        return Ok(());
    }

    let waiting_text = if action_type == "retell" {
        t!("speech.retell_wait", locale = &locale)
    } else {
        t!("speech.summary_wait", locale = &locale)
    };

    bot.edit_message_text(message.chat.id, message.id, waiting_text)
        .await?;

    let file_data_result = save_file_to_memory(&bot, &cache_entry.file_id, config).await;

    let new_summary_result = match file_data_result {
        Ok(file_data) => {
            summarize_audio(
                cache_entry.mime_type.clone(),
                file_data,
                config.clone(),
                action_type,
                settings.summary_model.api_key().to_string(),
            )
            .await
        }
        Err(e) => {
            error!("Failed to download file for summarization: {:?}", e);
            Err(e)
        }
    };

    match new_summary_result {
        Ok(new_summary)
            if !new_summary.is_empty() && !new_summary.contains("Не удалось получить") =>
        {
            cache_entry.summary = Some(new_summary.clone());
            cache.set(&file_cache_key, &cache_entry, 86400).await?;

            let text_parts = split_text(&new_summary, 4000);

            let icon = if action_type == "retell" {
                "📝"
            } else {
                "✨"
            };
            let final_text = format!(
                "{}:\n<blockquote expandable>{}</blockquote>",
                icon,
                html::escape(&text_parts[0])
            );
            let keyboard = create_summary_pagination_keyboard(0, text_parts.len(), &locale);

            bot.edit_message_text(message.chat.id, message.id, final_text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .await?;
        }
        _ => {
            let error_text = t!("speech.failed_summary", locale = &locale);
            let retry_keyboard =
                create_retry_keyboard(audio_message_id, action_type, cache_entry.attempt, &locale);

            bot.edit_message_text(message.chat.id, message.id, error_text)
                .reply_markup(retry_keyboard)
                .await?;
        }
    }

    Ok(())
}

async fn get_cached(
    bot: &Bot,
    file: &AudioStruct,
    config: &Config,
    force_no_cache: bool,
    model: String,
    locale: &str,
) -> Result<TranscriptionCache, MyError> {
    let cache = config.get_redis_client();
    let file_cache_key = format!("transcription_by_file:{}", &file.file_unique_id);

    if !force_no_cache
        && let Some(cached_text) = cache.get::<TranscriptionCache>(&file_cache_key).await?
        && !cached_text.full_text.is_empty()
    {
        debug!("File cache HIT for unique_id: {}", &file.file_unique_id);
        return Ok(cached_text);
    }

    let file_data = save_file_to_memory(bot, &file.file_id, config).await?;
    let transcription = Transcription {
        mime_type: file.mime_type.to_string(),
        data: file_data,
        config: config.clone(),
        custom_model: model,
    };

    let processed_parts = transcription.to_text(locale).await;
    if processed_parts.is_empty() || processed_parts[0].contains("❌") {
        let error_message = processed_parts.first().cloned().unwrap_or_default();
        return Err(BotError::Other(error_message).into());
    }

    let full_text = processed_parts.join("\n\n");
    let new_cache_entry = TranscriptionCache {
        full_text,
        summary: None,
        file_id: file.file_id.clone(),
        mime_type: file.mime_type.clone(),
        attempt: 0,
    };

    cache.set(&file_cache_key, &new_cache_entry, 86400).await?;
    Ok(new_cache_entry)
}

pub async fn retry_speech_handler(
    bot: Bot,
    query: CallbackQuery,
    config: &Config,
    _original_message_id: i32,
    action_type: &str,
    attempt: u32,
) -> Result<(), MyError> {
    let Some(message) = query
        .message
        .as_ref()
        .and_then(|m| m.regular_message().cloned())
    else {
        return Ok(());
    };
    let locale = get_chat_locale(&message.chat, config).await;

    bot.answer_callback_query(query.clone().id).await?;

    if attempt >= 1 {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            t!("speech.limit_exceeded", locale = &locale),
        )
        .await?;
        return Ok(());
    }

    let owner = Owner {
        id: message.chat.id.to_string(),
        r#type: if message.chat.is_private() {
            "user".to_string()
        } else {
            "group".to_string()
        },
    };
    let settings: SpeechRecognitionSettings = DbSettings::get_module_settings(&owner, "speech")
        .await
        .unwrap_or_default();

    let bot_message_id = message.id.0;
    let new_attempt = attempt + 1;

    let Some(replied_to_audio_message_id) = message.reply_to_message().map(|m| m.id.0) else {
        return Ok(());
    };
    let cache = config.get_redis_client();
    let message_file_map_key = format!("message_file_map:{}", bot_message_id);
    let Some(file_unique_id): Option<String> = cache.get::<String>(&message_file_map_key).await?
    else {
        return Ok(());
    };
    let file_cache_key = format!("transcription_by_file:{}", file_unique_id);

    if action_type == "transcribe" {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            t!("speech.re_processing", locale = &locale),
        )
        .await?;

        let Some(cache_entry_template): Option<TranscriptionCache> =
            cache.get(&file_cache_key).await?
        else {
            return Ok(());
        };
        let file = AudioStruct {
            mime_type: cache_entry_template.mime_type,
            file_id: cache_entry_template.file_id,
            file_unique_id: file_unique_id.clone(),
        };

        cache.delete(&file_cache_key).await?;

        match get_cached(
            &bot,
            &file,
            config,
            true,
            settings.transcription_model.api_key().to_string(),
            &locale,
        )
        .await
        {
            Ok(cache_entry) => {
                let text_parts = split_text(&cache_entry.full_text, 4000);
                let keyboard =
                    create_transcription_keyboard(0, text_parts.len(), query.from.id.0, &locale);
                bot.edit_message_text(
                    message.chat.id,
                    message.id,
                    format!(
                        "<blockquote expandable>{}</blockquote>",
                        html::escape(&text_parts[0])
                    ),
                )
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .await?;
            }
            Err(_) => {
                let retry_keyboard = create_retry_keyboard(
                    replied_to_audio_message_id,
                    "transcribe",
                    new_attempt,
                    &locale,
                );
                bot.edit_message_text(
                    message.chat.id,
                    message.id,
                    t!("speech.error_retry", locale = &locale),
                )
                .reply_markup(retry_keyboard)
                .await?;
            }
        }
    } else {
        let waiting_text = if action_type == "retell" {
            t!("speech.re_retell_wait", locale = &locale)
        } else {
            t!("speech.re_summary_wait", locale = &locale)
        };
        bot.edit_message_text(message.chat.id, message.id, waiting_text)
            .await?;

        let mut cache_entry = match cache.get::<TranscriptionCache>(&file_cache_key).await? {
            Some(entry) => entry,
            None => {
                bot.edit_message_text(
                    message.chat.id,
                    message.id,
                    t!("speech.audio_not_found", locale = &locale),
                )
                .await?;
                return Ok(());
            }
        };

        cache_entry.summary = None;
        let file_data_result = save_file_to_memory(&bot, &cache_entry.file_id, config).await;

        let new_summary_result = match file_data_result {
            Ok(file_data) => {
                summarize_audio(
                    cache_entry.mime_type.clone(),
                    file_data,
                    config.clone(),
                    action_type,
                    settings.summary_model.api_key().to_string(),
                )
                .await
            }
            Err(e) => Err(e),
        };

        match new_summary_result {
            Ok(new_summary)
                if !new_summary.is_empty() && !new_summary.contains("Не удалось получить") =>
            {
                cache_entry.summary = Some(new_summary.clone());
                cache_entry.attempt = 0;
                cache.set(&file_cache_key, &cache_entry, 86400).await?;

                let text_parts = split_text(&new_summary, 4000);
                let icon = if action_type == "retell" {
                    "📝"
                } else {
                    "✨"
                };
                let final_text = format!(
                    "{}:\n<blockquote expandable>{}</blockquote>",
                    icon,
                    html::escape(&text_parts[0])
                );
                let keyboard = create_summary_pagination_keyboard(0, text_parts.len(), &locale);
                bot.edit_message_text(message.chat.id, message.id, final_text)
                    .parse_mode(ParseMode::Html)
                    .reply_markup(keyboard)
                    .await?;
            }
            _ => {
                cache_entry.attempt = new_attempt;
                let retry_keyboard = create_retry_keyboard(
                    replied_to_audio_message_id,
                    action_type,
                    cache_entry.attempt,
                    &locale,
                );
                bot.edit_message_text(
                    message.chat.id,
                    message.id,
                    t!("speech.failed_summary", locale = &locale),
                )
                .reply_markup(retry_keyboard)
                .await?;
                cache.set(&file_cache_key, &cache_entry, 86400).await?;
            }
        }
    }
    Ok(())
}

pub async fn summarize_audio(
    mime_type: String,
    data: Bytes,
    config: Config,
    action_type: &str,
    model: String,
) -> Result<String, MyError> {
    let mut settings = Settings::new();
    settings.set_all_safety_settings(HarmBlockThreshold::BlockNone);

    let prompt = if action_type == "retell" {
        config.get_json_config().get_retell_prompt().to_owned()
    } else {
        config.get_json_config().get_summarize_prompt().to_owned()
    };
    settings.set_system_instruction(&prompt);

    let mut file_manager = FileManager::new();
    file_manager.set_base_url(config.get_gemini_base_url());

    let file_data = file_manager
        .add_file_from_bytes(
            "audio_summary",
            data.to_vec(),
            &mime_type,
            Some(Duration::from_secs(180)),
        )
        .await
        .map_err(MyError::from)?;

    let mut client = GemSession::Builder()
        .base_url(config.get_gemini_base_url())
        .model(Models::Custom(model))
        .timeout(Some(Duration::from_secs(120)))
        .build();

    let response = client
        .send_message_with_file(
            "Please process this file.",
            file_data,
            Role::User,
            &settings,
        )
        .await
        .map_err(MyError::from)?;

    Ok(response
        .get_results()
        .first()
        .cloned()
        .unwrap_or_else(|| "❌ Не удалось получить результат.".to_string()))
}

pub async fn get_file_id(msg: &Message) -> Option<AudioStruct> {
    match &msg.kind {
        MessageKind::Common(common) => match &common.media_kind {
            teloxide::types::MediaKind::Audio(audio) => Some(AudioStruct {
                mime_type: audio.audio.mime_type.as_ref()?.essence_str().to_owned(),
                file_id: audio.audio.file.id.0.to_string(),
                file_unique_id: audio.audio.file.unique_id.0.to_string(),
            }),
            teloxide::types::MediaKind::Voice(voice) => Some(AudioStruct {
                mime_type: voice.voice.mime_type.as_ref()?.essence_str().to_owned(),
                file_id: voice.voice.file.id.0.to_owned(),
                file_unique_id: voice.voice.file.unique_id.0.to_owned(),
            }),
            teloxide::types::MediaKind::VideoNote(video_note) => Some(AudioStruct {
                mime_type: "video/mp4".to_owned(),
                file_id: video_note.video_note.file.id.0.to_owned(),
                file_unique_id: video_note.video_note.file.unique_id.0.to_owned(),
            }),
            _ => None,
        },
        _ => None,
    }
}

pub async fn save_file_to_memory(
    bot: &Bot,
    file_id: &str,
    config: &Config,
) -> Result<Bytes, MyError> {
    let file = bot.get_file(FileId(file_id.to_string())).send().await?;

    if std::path::Path::new(&file.path).exists()
        && let Ok(bytes) = std::fs::read(&file.path)
    {
        return Ok(Bytes::from(bytes));
    }

    let token = bot.token();
    let relative_path = file
        .path
        .split_once(token)
        .map(|(_, rest)| rest.trim_start_matches('/'))
        .unwrap_or(&file.path);

    let file_url = format!(
        "{}/file/bot{}/{}",
        config.get_telegram_api(),
        token,
        relative_path
    );

    let response = reqwest::get(&file_url).await?;

    if !response.status().is_success() {
        return Err(anyhow!("HTTP Error: {}", response.status()));
    }

    Ok(response.bytes().await?)
}

impl Transcription {
    pub async fn to_text(&self, locale: &str) -> Vec<String> {
        let mut settings = Settings::new();
        settings.set_all_safety_settings(HarmBlockThreshold::BlockNone);

        let error_answer = t!("speech.error_transcription", locale = locale);
        let prompt = self.config.get_json_config().get_ai_prompt().to_owned();
        settings.set_system_instruction(&prompt);

        let mut file_manager = FileManager::new();
        file_manager.set_base_url(self.config.get_gemini_base_url());

        let file_data = match file_manager
            .add_file_from_bytes(
                "audio_transcription",
                self.data.to_vec(),
                &self.mime_type,
                Some(Duration::from_secs(180)),
            )
            .await
        {
            Ok(fd) => fd,
            Err(e) => {
                return vec![format!("❌ Ошибка загрузки файла: {}", e)];
            }
        };

        let mut client = GemSession::Builder()
            .base_url(self.config.get_gemini_base_url())
            .model(Models::Custom(self.custom_model.to_string()))
            .timeout(Some(Duration::from_secs(120)))
            .build();

        let mut attempts = 0;
        let mut last_error = String::new();

        let re = Regex::new(r"https?://[^\s)]+").unwrap();
        while attempts < 3 {
            match client
                .send_message_with_file(
                    "Please transcribe this audio.",
                    file_data.clone(),
                    Role::User,
                    &settings,
                )
                .await
            {
                Ok(response) => {
                    let full_text = response.get_results().first().cloned().unwrap_or_default();
                    if !full_text.is_empty() {
                        return split_text(&full_text, 4000);
                    }
                    attempts += 1;
                    info!("Received empty response, attempt {}", attempts);
                }
                Err(error) => {
                    attempts += 1;
                    let error_string = error.to_string();
                    error!(
                        "Transcription error (attempt {}): {:?}",
                        attempts, error_string
                    );

                    let safe_error = re
                        .replace_all(&error_string, "<code>[Gemini URL]</code>")
                        .to_string();

                    if safe_error == last_error {
                        continue;
                    }
                    last_error = safe_error;
                }
            }
        }
        vec![error_answer.to_string() + "\n\n" + &last_error]
    }
}
