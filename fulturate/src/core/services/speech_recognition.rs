use crate::{
    bot::{
        keyboards::transcription::{
            create_retry_keyboard, create_summary_keyboard, create_summary_pagination_keyboard,
            create_transcription_keyboard,
        },
        modules::{Owner, speech_recognition::SpeechRecognitionSettings},
    },
    core::{config::Config, db::schemas::settings::Settings as DbSettings},
    errors::MyError,
    util::{enums::AudioStruct, split_text},
};
use bytes::Bytes;
use gem_rs::{
    api::Models,
    client::GemSession,
    types::{FileManager, HarmBlockThreshold, Role, Settings},
};
use log::{debug, error, info};
use redis_macros::{FromRedisValue, ToRedisArgs};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use teloxide::{
    prelude::*,
    types::{FileId, MessageKind, ParseMode, ReplyParameters},
    utils::html,
};

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
        bot.edit_message_text(message.chat.id, message.id, "❌ Кнопка устарела.")
            .await?;
        return Ok(());
    };

    let file_cache_key = format!("transcription_by_file:{}", file_unique_id);
    let Some(cache_entry) = cache.get::<TranscriptionCache>(&file_cache_key).await? else {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            "❌ Не удалось найти текст в кеше.",
        )
        .await?;
        return Ok(());
    };

    let full_text_source = if mode == "summary" {
        cache_entry
            .summary
            .unwrap_or_else(|| "Ошибка: краткое содержание не найдено.".to_string())
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
        create_summary_pagination_keyboard(page, text_parts.len())
    } else {
        create_transcription_keyboard(page, text_parts.len(), query.from.id.0)
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

    let cache = config.get_redis_client();
    let message_cache_key = format!("message_file_map:{}", message.id);
    let Some(file_unique_id) = cache.get::<String>(&message_cache_key).await? else {
        bot.edit_message_text(
            message.chat.id,
            message.id,
            "❌ Не удалось найти исходное сообщение.",
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
            "❌ Не удалось найти текст в кеше.",
        )
        .await?;
        return Ok(());
    };

    let text_parts = split_text(&cache_entry.full_text, 4000);
    let keyboard = create_transcription_keyboard(0, text_parts.len(), query.from.id.0);

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
            .text("❌ Не удалось найти исходное сообщение для повторной попытки.")
            .show_alert(true)
            .await?;
        return Ok(());
    };

    let cache = config.get_redis_client();
    let message_file_map_key = format!("message_file_map:{}", message.id);
    let Some(file_unique_id) = cache.get::<String>(&message_file_map_key).await? else {
        bot.edit_message_text(message.chat.id, message.id, "❌ Кнопка устарела.")
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
                "❌ Не удалось найти исходное аудио.",
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
        let keyboard = create_summary_pagination_keyboard(0, text_parts.len());

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
            "❌ Нельзя составить краткое содержание из аудио без речи.",
        )
        .parse_mode(ParseMode::Html)
        .reply_markup(create_summary_keyboard())
        .await?;
        return Ok(());
    }

    let waiting_text = if action_type == "retell" {
        "📝 Пишу краткий пересказ..."
    } else {
        "✨ Подвожу итоги..."
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
            let keyboard = create_summary_pagination_keyboard(0, text_parts.len());

            bot.edit_message_text(message.chat.id, message.id, final_text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .await?;
        }
        _ => {
            let error_text = "❌ Не удалось составить краткое содержание.";
            let retry_keyboard =
                create_retry_keyboard(audio_message_id, action_type, cache_entry.attempt);

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
    let processed_parts = transcription.to_text().await;

    if processed_parts.is_empty() || processed_parts[0].contains("Не удалось преобразовать")
    {
        let error_message = processed_parts.first().cloned().unwrap_or_default();
        return Err(MyError::Other(error_message));
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
    debug!(
        "Saved new transcription to file cache for unique_id: {}",
        file.file_unique_id
    );

    Ok(new_cache_entry)
}

pub async fn transcription_handler(
    bot: Bot,
    msg: &Message,
    config: &Config,
) -> Result<(), MyError> {
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

    let message = bot
        .send_message(msg.chat.id, "Обрабатываю аудио...")
        .reply_parameters(ReplyParameters::new(msg.id))
        .parse_mode(ParseMode::Html)
        .await
        .ok();

    let Some(message) = message else {
        return Ok(());
    };
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };

    if let Some(file) = get_file_id(msg).await {
        let cache = config.get_redis_client();
        let message_file_map_key = format!("message_file_map:{}", message.id);

        cache
            .set(&message_file_map_key, &file.file_unique_id, 86400)
            .await?;

        let file_cache_key = format!("transcription_by_file:{}", &file.file_unique_id);
        let empty_cache = TranscriptionCache {
            full_text: String::new(),
            summary: None,
            file_id: file.file_id.clone(),
            mime_type: file.mime_type.clone(),
            attempt: 0,
        };
        cache.set(&file_cache_key, &empty_cache, 86400).await?;

        let model_key = settings.transcription_model.api_key().to_string();

        match get_cached(&bot, &file, config, false, model_key).await {
            Ok(cache_entry) => {
                let text_parts = split_text(&cache_entry.full_text, 4000);
                if text_parts.is_empty() {
                    return Ok(());
                }

                let keyboard = create_transcription_keyboard(0, text_parts.len(), user.id.0);
                bot.edit_message_text(
                    msg.chat.id,
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
            Err(e) => {
                error!("Failed to get transcription: {:?}", e);
                let error_text = "❌ Произошла ошибка при обработке аудио.".to_string();
                let retry_keyboard = create_retry_keyboard(msg.id.0, "transcribe", 0);
                bot.edit_message_text(message.chat.id, message.id, error_text)
                    .reply_markup(retry_keyboard)
                    .await?;
            }
        }
    } else {
        bot.edit_message_text(message.chat.id, message.id, "❌ Не удалось найти аудио.")
            .await?;
    }
    Ok(())
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

    bot.answer_callback_query(query.clone().id).await?;

    if attempt >= 1 {
        bot.edit_message_text(message.chat.id, message.id, "❌ Лимит попыток исчерпан.")
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
            "🔁 Повторная обработка аудио...",
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
        )
        .await
        {
            Ok(cache_entry) => {
                let text_parts = split_text(&cache_entry.full_text, 4000);
                let keyboard = create_transcription_keyboard(0, text_parts.len(), query.from.id.0);
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
                let retry_keyboard =
                    create_retry_keyboard(replied_to_audio_message_id, "transcribe", new_attempt);
                bot.edit_message_text(message.chat.id, message.id, "❌ Ошибка при повторе.")
                    .reply_markup(retry_keyboard)
                    .await?;
            }
        }
    } else {
        let waiting_text = if action_type == "retell" {
            "🔁 Повторный пересказ..."
        } else {
            "🔁 Повторные итоги..."
        };
        bot.edit_message_text(message.chat.id, message.id, waiting_text)
            .await?;

        let mut cache_entry = match cache.get::<TranscriptionCache>(&file_cache_key).await? {
            Some(entry) => entry,
            None => {
                bot.edit_message_text(message.chat.id, message.id, "❌ Исходное аудио не найдено.")
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
                let keyboard = create_summary_pagination_keyboard(0, text_parts.len());
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
                );
                bot.edit_message_text(message.chat.id, message.id, "❌ Не удалось.")
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

    let file_manager = FileManager::new();

    let file_data = file_manager
        .add_file_from_bytes(
            "audio_summary",
            data.to_vec(),
            &mime_type,
            Some(std::time::Duration::from_secs(180)),
        )
        .await
        .map_err(|e| MyError::from(e))?;

    let mut client = GemSession::Builder()
        .model(Models::Custom(model))
        .timeout(Some(std::time::Duration::from_secs(120)))
        .build();

    let response = client
        .send_message_with_file(
            "Please process this file.",
            file_data,
            Role::User,
            &settings,
        )
        .await
        .map_err(|e| MyError::from(e))?;

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

    if std::path::Path::new(&file.path).exists() {
        if let Ok(bytes) = std::fs::read(&file.path) {
            return Ok(Bytes::from(bytes));
        }
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
        return Err(MyError::Other(format!("HTTP Error: {}", response.status())));
    }

    Ok(response.bytes().await?)
}

impl Transcription {
    pub async fn to_text(&self) -> Vec<String> {
        let mut settings = Settings::new();
        settings.set_all_safety_settings(HarmBlockThreshold::BlockNone);

        let error_answer = "❌ Не удалось преобразовать текст из сообщения.".to_string();
        // let ai_model = self.config.get_json_config().get_ai_model().to_owned();
        let prompt = self.config.get_json_config().get_ai_prompt().to_owned();
        settings.set_system_instruction(&prompt);

        let file_manager = FileManager::new();

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
            Err(e) => return vec![format!("❌ Ошибка загрузки файла: {}", e)],
        };

        let mut client = GemSession::Builder()
            .model(Models::Custom(self.custom_model.to_string()))
            .timeout(Some(Duration::from_secs(120)))
            .build();

        let mut attempts = 0;
        let mut last_error = String::new();

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
                    if error_string == last_error {
                        continue;
                    }
                    last_error = error_string;
                    error!("Transcription error (attempt {}): {:?}", attempts, error);
                }
            }
        }
        vec![error_answer + "\n\n" + &last_error]
    }
}
