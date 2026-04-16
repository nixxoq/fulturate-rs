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
        metrics::{ERRORS_COUNTER, MODULE_USAGE},
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
    errors::GemErrorKind,
    types::{
        BatchOperation, BatchState, FileData, FileManager, HarmBlockThreshold, Role, Settings,
        group_batch_jsonl_by_key, parse_batch_jsonl_lines,
    },
};
use log::{error, info, warn};
use redis::{AsyncCommands, Script};
use redis_macros::{FromRedisValue, ToRedisArgs};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
use teloxide::{
    prelude::*,
    types::{FileId, MessageId, MessageKind, ParseMode, ReplyParameters},
    utils::html,
};
use uuid::Uuid;

const QUEUE_LIMIT_FREE: usize = 2;
const QUEUE_LIMIT_PREMIUM: usize = 50;
const REDIS_QUEUE_FREE: &str = "sr_queue:free";
const REDIS_QUEUE_PREMIUM: &str = "sr_queue:premium";
const REDIS_WORKER_BUSY: &str = "sr_worker_active";
const REDIS_PENDING_SET_PREFIX: &str = "sr_pending_set";
const REDIS_PENDING_JOB_PREFIX: &str = "sr_pending_job";
const REDIS_BATCH_PENDING_PREFIX: &str = "sr_batch_pending";
const PENDING_SLOT_TTL_SECS: usize = 60 * 60;
const PENDING_RECONCILE_INTERVAL_SECS: u64 = 60;
const QUEUE_GLOBAL_SOFT_CAP: usize = 200;
const BATCH_DURATION_THRESHOLD_SECS: u32 = 30 * 60;
const AUDIO_CHUNK_SECONDS: u32 = 480;
const FILE_UPLOAD_TIMEOUT_SECS: u64 = 900;
const GENERATE_TIMEOUT_SECS: u64 = 900;
const BATCH_POLL_INTERVAL_SECS: u64 = 5;
const BATCH_POLL_TIMEOUT_SECS: u64 = 60 * 60;
const BATCH_REQUEST_MIME: &str = "application/x-ndjson";
const BATCH_REQUEST_PROMPT_TRANSCRIBE: &str =
    "Please transcribe this audio chunk verbatim and return plain text only.";
const DEBUG_MESSAGE_MAX_CHARS: usize = 700;
const BATCH_SUBMIT_MAX_ATTEMPTS: usize = 4;
const SYNC_TRANSCRIBE_MAX_ATTEMPTS: usize = 4;

#[derive(Serialize, Deserialize, Debug)]
pub struct SpeechJob {
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub owner_key: String,
    pub chat_id: ChatId,
    pub message_id: MessageId,
    pub status_message_id: MessageId,
    pub user_id: u64,
    pub file_info: AudioStruct,
    pub settings: SpeechRecognitionSettings,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub batch_operation_name: Option<String>,
    #[serde(default)]
    pub draft_id: u64,
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
    pub(crate) duration_seconds: Option<u32>,
    pub(crate) chat_id: Option<ChatId>,
    pub(crate) draft_id: Option<u64>,
}

fn pending_set_key(owner_key: &str) -> String {
    format!("{}:{}", REDIS_PENDING_SET_PREFIX, owner_key)
}

fn pending_job_key(job_id: &str) -> String {
    format!("{}:{}", REDIS_PENDING_JOB_PREFIX, job_id)
}

fn batch_pending_key(job_id: &str) -> String {
    format!("{}:{}", REDIS_BATCH_PENDING_PREFIX, job_id)
}

fn derive_owner_key(msg: &Message) -> Option<String> {
    if let Some(user) = msg.from.as_ref() {
        return Some(format!("user:{}", user.id.0));
    }

    msg.sender_chat
        .as_ref()
        .map(|chat| format!("sender_chat:{}", chat.id.0))
}

fn draft_id_from_job(job_id: &str) -> u64 {
    let digest = md5::compute(job_id);
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[0..8]);
    let value = u64::from_be_bytes(bytes);
    if value == 0 { 1 } else { value }
}

async fn send_draft_progress(bot: &Bot, chat_id: ChatId, draft_id: u64, text: String) {
    let _ = bot
        .send_message_draft(chat_id, draft_id, text)
        .parse_mode(ParseMode::Html)
        .await;
}

fn compact_debug_text(text: &str, limit: usize) -> String {
    let normalized = redact_sensitive_text(text)
        .replace(['\r', '\n', '\t'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if normalized.chars().count() <= limit {
        return normalized;
    }

    let mut compact = normalized.chars().take(limit).collect::<String>();
    compact.push_str("...");
    compact
}

fn redact_sensitive_text(text: &str) -> String {
    let key_regex = Regex::new(r"(key=)[^&\s)]+").expect("key redaction regex must compile");
    key_regex.replace_all(text, "${1}[REDACTED]").to_string()
}

fn is_retryable_error_text(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("resource_exhausted")
        || lower.contains("resource has been exhausted")
        || lower.contains("(unavailable)")
        || lower.contains("high demand")
        || lower.contains("connection error")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("status code: 429")
        || lower.contains("status code: 503")
}

fn retry_backoff_seconds(attempt: usize) -> u64 {
    match attempt {
        1 => 5,
        2 => 12,
        3 => 25,
        _ => 40,
    }
}

async fn send_draft_debug(
    bot: &Bot,
    chat_id: Option<ChatId>,
    draft_id: Option<u64>,
    stage: &str,
    details: &str,
) {
    if let (Some(chat_id), Some(draft_id)) = (chat_id, draft_id) {
        let stage = html::escape(stage);
        let details = html::escape(&compact_debug_text(details, DEBUG_MESSAGE_MAX_CHARS));
        send_draft_progress(
            bot,
            chat_id,
            draft_id,
            format!(
                "⚠️ <b>debug</b> <code>{}</code>\n<code>{}</code>",
                stage, details
            ),
        )
        .await;
    }
}

async fn cleanup_owner_pending_stale(
    con: &mut redis::aio::MultiplexedConnection,
    owner_key: &str,
) -> Result<usize, MyError> {
    let set_key = pending_set_key(owner_key);
    let members: Vec<String> = con.smembers(&set_key).await.unwrap_or_default();
    let mut stale_members = Vec::new();

    for member in members {
        let meta_key = pending_job_key(&member);
        let exists: usize = con.exists(meta_key).await.unwrap_or(0);
        if exists == 0 {
            stale_members.push(member);
        }
    }

    if !stale_members.is_empty() {
        let _: () = con.srem(&set_key, stale_members).await?;
    }

    let count: usize = con.scard(&set_key).await.unwrap_or(0);
    if count == 0 {
        let _: () = con.del(&set_key).await.unwrap_or_default();
    }

    Ok(count)
}

async fn enqueue_job_with_quota(
    con: &mut redis::aio::MultiplexedConnection,
    owner_key: &str,
    queue_key: &str,
    limit: usize,
    job_id: &str,
    job_json: &str,
) -> Result<(bool, usize), MyError> {
    let set_key = pending_set_key(owner_key);
    let job_key = pending_job_key(job_id);

    let script = Script::new(
        r#"
            local current = redis.call('SCARD', KEYS[1])
            if tonumber(current) >= tonumber(ARGV[1]) then
                return {0, current}
            end

            redis.call('SADD', KEYS[1], ARGV[2])
            redis.call('EXPIRE', KEYS[1], tonumber(ARGV[4]))
            redis.call('SETEX', KEYS[2], tonumber(ARGV[4]), ARGV[3])
            redis.call('RPUSH', KEYS[3], ARGV[5])
            local updated = redis.call('SCARD', KEYS[1])
            return {1, updated}
        "#,
    );

    let result: Vec<i64> = script
        .key(set_key)
        .key(job_key)
        .key(queue_key)
        .arg(limit as i64)
        .arg(job_id)
        .arg(owner_key)
        .arg(PENDING_SLOT_TTL_SECS as i64)
        .arg(job_json)
        .invoke_async(con)
        .await?;

    if result.len() != 2 {
        return Ok((false, limit));
    }

    Ok((result[0] == 1, result[1].max(0) as usize))
}

async fn release_pending_slot(
    con: &mut redis::aio::MultiplexedConnection,
    owner_key: &str,
    job_id: &str,
) -> Result<(), MyError> {
    let set_key = pending_set_key(owner_key);
    let job_key = pending_job_key(job_id);
    let _: () = con.srem(set_key.clone(), job_id).await.unwrap_or_default();
    let _: () = con.del(job_key).await.unwrap_or_default();

    let count: usize = con.scard(&set_key).await.unwrap_or(0);
    if count == 0 {
        let _: () = con.del(set_key).await.unwrap_or_default();
    }

    Ok(())
}

async fn reconcile_pending_slots(
    con: &mut redis::aio::MultiplexedConnection,
) -> Result<(), MyError> {
    let set_keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{}:*", REDIS_PENDING_SET_PREFIX))
        .query_async(con)
        .await
        .unwrap_or_default();

    for set_key in set_keys {
        let owner_key = set_key
            .strip_prefix(&(REDIS_PENDING_SET_PREFIX.to_string() + ":"))
            .unwrap_or_default();
        if owner_key.is_empty() {
            continue;
        }
        let _ = cleanup_owner_pending_stale(con, owner_key).await;
    }

    Ok(())
}

fn should_use_batch(duration_seconds: Option<u32>) -> bool {
    duration_seconds
        .map(|duration| duration >= BATCH_DURATION_THRESHOLD_SECS)
        .unwrap_or(false)
}

fn classify_error_code(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") {
        return "generation_timeout";
    }
    if lower.contains("max_tokens") || lower.contains("max tokens") {
        return "max_tokens";
    }
    if lower.contains("upload") {
        return "upload_failed";
    }
    if lower.contains("batch") {
        return "batch_failed";
    }
    "unknown"
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
    let mut last_reconcile = Instant::now();

    loop {
        if last_reconcile.elapsed() >= Duration::from_secs(PENDING_RECONCILE_INTERVAL_SECS) {
            if let Err(err) = reconcile_pending_slots(&mut con).await {
                warn!("Failed to reconcile pending speech slots: {}", err);
            }
            last_reconcile = Instant::now();
        }

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
            let parsed_meta = serde_json::from_str::<serde_json::Value>(&json).ok();
            match serde_json::from_str::<SpeechJob>(&json) {
                Ok(mut job) => {
                    let bot_clone = bot.clone();
                    let config_clone = config.clone();

                    if job.job_id.is_empty() {
                        job.job_id = format!("legacy:{}:{}", job.chat_id.0, job.message_id.0);
                    }
                    if job.owner_key.is_empty() {
                        job.owner_key = format!("user:{}", job.user_id);
                    }
                    if job.draft_id == 0 {
                        job.draft_id = draft_id_from_job(&job.job_id);
                    }
                    let owner_key = job.owner_key.clone();
                    let job_id = job.job_id.clone();

                    let _: () = con
                        .set_ex(REDIS_WORKER_BUSY, 1, 300)
                        .await
                        .unwrap_or_default();

                    let res = process_speech_job(bot_clone, config_clone, job).await;
                    if let Err(e) = res {
                        error!("Failed to process speech job: {}", e);
                    }

                    let _: () = con.del(REDIS_WORKER_BUSY).await.unwrap_or_default();
                    let _ = release_pending_slot(&mut con, &owner_key, &job_id).await;
                }
                Err(e) => {
                    error!("Failed to deserialize speech job: {}", e);
                    if let Some(meta) = parsed_meta {
                        let owner_key = meta
                            .get("owner_key")
                            .and_then(|v| v.as_str())
                            .map(ToOwned::to_owned)
                            .or_else(|| {
                                meta.get("user_id")
                                    .and_then(|v| v.as_u64())
                                    .map(|id| format!("user:{}", id))
                            });
                        let job_id = meta
                            .get("job_id")
                            .and_then(|v| v.as_str())
                            .map(ToOwned::to_owned);

                        if let (Some(owner_key), Some(job_id)) = (owner_key, job_id) {
                            let _ = release_pending_slot(&mut con, &owner_key, &job_id).await;
                        }
                    }
                }
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

    let _ = bot
        .edit_message_text(
            job.chat_id,
            job.status_message_id,
            t!("speech.processing_started", locale = &locale),
        )
        .await;
    if job.draft_id != 0 {
        send_draft_progress(
            &bot,
            job.chat_id,
            job.draft_id,
            "🔄 <i>processing_started</i>".to_string(),
        )
        .await;
    }

    let cache = config.get_redis_client();

    let message_file_map_key = format!("message_file_map:{}", job.status_message_id);
    cache
        .set(&message_file_map_key, &job.file_info.file_unique_id, 86400)
        .await?;

    let model_key = job.settings.transcription_model.api_key().to_string();

    match get_cached(
        &bot,
        &job.file_info,
        &config,
        false,
        model_key,
        &locale,
        Some(job.chat_id),
        Some(job.draft_id),
    )
    .await
    {
        Ok(cache_entry) => {
            let text_parts = split_text(&cache_entry.full_text, 4000);
            if text_parts.is_empty() {
                bot.edit_message_text(
                    job.chat_id,
                    job.status_message_id,
                    t!("speech.error_empty", locale = &locale),
                )
                .await?;
                return Ok(());
            }

            MODULE_USAGE
                .with_label_values(&["speech_recognition", "transcribe"])
                .inc();

            let keyboard = create_transcription_keyboard(0, text_parts.len(), job.user_id, &locale);

            bot.edit_message_text(
                job.chat_id,
                job.status_message_id,
                format!(
                    "<blockquote expandable>{}</blockquote>",
                    html::escape(&text_parts[0])
                ),
            )
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await?;

            if job.draft_id != 0 {
                send_draft_progress(
                    &bot,
                    job.chat_id,
                    job.draft_id,
                    "✅ <i>processing_completed</i>".to_string(),
                )
                .await;
            }
        }
        Err(e) => {
            ERRORS_COUNTER
                .with_label_values(&["error.speech_recognition"])
                .inc();
            error!("Processing failed for chat {}: {:?}", job.chat_id, e);
            let error_code = classify_error_code(&e.to_string());
            let error_text = format!(
                "{} (code: {})",
                t!("speech.error_processing", locale = &locale),
                error_code
            );
            let retry_keyboard = create_retry_keyboard(job.message_id.0, "transcribe", 0, &locale);

            bot.edit_message_text(job.chat_id, job.status_message_id, error_text)
                .reply_markup(retry_keyboard)
                .await?;

            if job.draft_id != 0 {
                send_draft_progress(
                    &bot,
                    job.chat_id,
                    job.draft_id,
                    format!("❌ <i>processing_failed</i> ({})", error_code),
                )
                .await;
            }
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

    let Some(owner_key) = derive_owner_key(msg) else {
        return Ok(());
    };
    let requester_id = msg.from.as_ref().map(|user| user.id.0).unwrap_or(0);
    let is_premium = if requester_id == 0 {
        false
    } else {
        User::check_premium(&requester_id.to_string()).await
    };

    if let Some(file_info) = get_file_id(msg).await {
        let cache_client = config.get_redis_client();
        let file_cache_key = format!("transcription_by_file:{}", &file_info.file_unique_id);

        if let Ok(Some(cached)) = cache_client
            .get::<TranscriptionCache>(&file_cache_key)
            .await
            && !cached.full_text.is_empty()
        {
            MODULE_USAGE
                .with_label_values(&["speech_recognition", "transcribe"])
                .inc();
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
                    requester_id,
                    &locale,
                ))
                .await?;

            let message_file_map_key = format!("message_file_map:{}", sent_msg.id);
            cache_client
                .set(&message_file_map_key, &file_info.file_unique_id, 86400)
                .await?;

            return Ok(());
        }

        let mut con = config
            .get_redis_client()
            .client
            .get_multiplexed_tokio_connection()
            .await?;

        let current_user_jobs = cleanup_owner_pending_stale(&mut con, &owner_key).await?;

        let limit = if is_premium {
            QUEUE_LIMIT_PREMIUM
        } else {
            QUEUE_LIMIT_FREE
        };

        if current_user_jobs >= limit {
            bot.send_message(
                msg.chat.id,
                t!(
                    "speech.queue_full",
                    locale = &locale,
                    count = current_user_jobs,
                    limit = limit
                ),
            )
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
            return Ok(());
        }

        let premium_len: usize = con.llen(REDIS_QUEUE_PREMIUM).await.unwrap_or(0);
        let free_len: usize = con.llen(REDIS_QUEUE_FREE).await.unwrap_or(0);
        let is_busy: usize = con.exists(REDIS_WORKER_BUSY).await.unwrap_or(0);
        let total_global = premium_len + free_len + is_busy;
        if total_global >= QUEUE_GLOBAL_SOFT_CAP {
            warn!(
                "Speech queue soft cap reached: {} (premium={}, free={}, busy={})",
                total_global, premium_len, free_len, is_busy
            );
        }

        let queue_key = if is_premium {
            REDIS_QUEUE_PREMIUM
        } else {
            REDIS_QUEUE_FREE
        };

        let pos = if is_premium {
            premium_len + is_busy + 1
        } else {
            premium_len + free_len + is_busy + 1
        };

        let status_msg = bot
            .send_message(
                msg.chat.id,
                t!("speech.added_to_queue", locale = &locale, pos = pos),
            )
            .reply_parameters(ReplyParameters::new(msg.id))
            .parse_mode(ParseMode::Html)
            .await?;

        let job_id = Uuid::new_v4().to_string();
        let draft_id = draft_id_from_job(&job_id);
        let job = SpeechJob {
            job_id: job_id.clone(),
            owner_key: owner_key.clone(),
            chat_id: msg.chat.id,
            message_id: msg.id,
            status_message_id: status_msg.id,
            user_id: requester_id,
            file_info,
            settings,
            mode: "sync".to_string(),
            batch_operation_name: None,
            draft_id,
        };

        let job_json = serde_json::to_string(&job)?;
        let (enqueued, current_count) =
            enqueue_job_with_quota(&mut con, &owner_key, queue_key, limit, &job_id, &job_json)
                .await?;

        if !enqueued {
            bot.edit_message_text(
                msg.chat.id,
                status_msg.id,
                t!(
                    "speech.queue_full",
                    locale = &locale,
                    count = current_count,
                    limit = limit
                ),
            )
            .await?;
            return Ok(());
        }

        send_draft_progress(
            &bot,
            msg.chat.id,
            draft_id,
            format!(
                "⏳ <i>{}</i>",
                t!("speech.added_to_queue", locale = &locale, pos = pos)
            ),
        )
        .await;
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
        let action = if action_type == "retell" {
            "retell"
        } else {
            "summary"
        };
        MODULE_USAGE
            .with_label_values(&["speech_recognition", action])
            .inc();

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
            let action = if action_type == "retell" {
                "retell"
            } else {
                "summary"
            };
            MODULE_USAGE
                .with_label_values(&["speech_recognition", action])
                .inc();

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
            let err_label = if action_type == "retell" {
                "error.speech_recognition_retell"
            } else {
                "error.speech_recognition_summary"
            };
            ERRORS_COUNTER.with_label_values(&[err_label]).inc();

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
    chat_id: Option<ChatId>,
    draft_id: Option<u64>,
) -> Result<TranscriptionCache, MyError> {
    let cache = config.get_redis_client();
    let file_cache_key = format!("transcription_by_file:{}", &file.file_unique_id);

    if !force_no_cache
        && let Some(cached_text) = cache.get::<TranscriptionCache>(&file_cache_key).await?
        && !cached_text.full_text.is_empty()
    {
        return Ok(cached_text);
    }

    let file_data = save_file_to_memory(bot, &file.file_id, config).await?;
    let transcription = Transcription {
        mime_type: file.mime_type.to_string(),
        data: file_data,
        config: config.clone(),
        custom_model: model,
        duration_seconds: file.duration_seconds,
        chat_id,
        draft_id,
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
            duration_seconds: None,
        };

        cache.delete(&file_cache_key).await?;

        match get_cached(
            &bot,
            &file,
            config,
            true,
            settings.transcription_model.api_key().to_string(),
            &locale,
            Some(message.chat.id),
            Some(draft_id_from_job(&format!(
                "retry:{}:{}",
                message.chat.id.0, message.id.0
            ))),
        )
        .await
        {
            Ok(cache_entry) => {
                MODULE_USAGE
                    .with_label_values(&["speech_recognition", "transcribe"])
                    .inc();
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
                ERRORS_COUNTER
                    .with_label_values(&["error.speech_recognition"])
                    .inc();
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
                let action = if action_type == "retell" {
                    "retell"
                } else {
                    "summary"
                };
                MODULE_USAGE
                    .with_label_values(&["speech_recognition", action])
                    .inc();

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
                let err_label = if action_type == "retell" {
                    "error.speech_recognition_retell"
                } else {
                    "error.speech_recognition_summary"
                };
                ERRORS_COUNTER.with_label_values(&[err_label]).inc();

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
    if data.len() > 12 * 1024 * 1024 {
        match summarize_audio_with_batch(&mime_type, &data, &config, action_type, &model).await {
            Ok(result) => return Ok(result),
            Err(err) => {
                warn!(
                    "Batch summary failed, fallback to sync: {}",
                    redact_sensitive_text(&err.to_string())
                );
            }
        }
    }

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
            Some(Duration::from_secs(FILE_UPLOAD_TIMEOUT_SECS)),
        )
        .await
        .map_err(MyError::from)?;

    let mut client = GemSession::Builder()
        .base_url(config.get_gemini_base_url())
        .model(Models::Custom(model))
        .timeout(Some(Duration::from_secs(GENERATE_TIMEOUT_SECS)))
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
                duration_seconds: Some(audio.audio.duration.seconds()),
            }),
            teloxide::types::MediaKind::Voice(voice) => Some(AudioStruct {
                mime_type: voice.voice.mime_type.as_ref()?.essence_str().to_owned(),
                file_id: voice.voice.file.id.0.to_owned(),
                file_unique_id: voice.voice.file.unique_id.0.to_owned(),
                duration_seconds: Some(voice.voice.duration.seconds()),
            }),
            teloxide::types::MediaKind::VideoNote(video_note) => Some(AudioStruct {
                mime_type: "video/mp4".to_owned(),
                file_id: video_note.video_note.file.id.0.to_owned(),
                file_unique_id: video_note.video_note.file.unique_id.0.to_owned(),
                duration_seconds: Some(video_note.video_note.duration.seconds()),
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

fn mime_to_extension(mime_type: &str) -> &'static str {
    match mime_type {
        "audio/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mp4" => "m4a",
        "video/mp4" => "mp4",
        _ => "bin",
    }
}

fn split_audio_to_chunks(
    data: &[u8],
    mime_type: &str,
    chunk_seconds: u32,
) -> Result<Vec<Vec<u8>>, MyError> {
    let temp_root = std::env::temp_dir().join(format!("sr_batch_{}", Uuid::new_v4()));
    fs::create_dir_all(&temp_root)?;

    let input_path = temp_root.join(format!("input.{}", mime_to_extension(mime_type)));
    fs::write(&input_path, data)?;

    let output_pattern = temp_root.join("chunk_%05d.wav");
    let output_pattern_str = output_pattern
        .to_str()
        .ok_or_else(|| anyhow!("Failed to build output pattern path"))?;

    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-i",
            input_path.to_str().unwrap_or_default(),
            "-f",
            "segment",
            "-segment_time",
            &chunk_seconds.to_string(),
            "-ac",
            "1",
            "-ar",
            "16000",
            output_pattern_str,
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let _ = fs::remove_dir_all(&temp_root);
        return Err(anyhow!("ffmpeg split failed: {}", stderr).into());
    }

    let mut files: Vec<PathBuf> = fs::read_dir(&temp_root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("chunk_"))
        })
        .collect();

    files.sort();
    let mut chunks = Vec::new();
    for file in files {
        chunks.push(fs::read(file)?);
    }

    let _ = fs::remove_dir_all(&temp_root);
    if chunks.is_empty() {
        return Err(anyhow!("No audio chunks were generated").into());
    }

    Ok(chunks)
}

async fn extract_response_file_name(operation: &BatchOperation) -> Option<String> {
    operation
        .batch()
        .and_then(|batch| batch.output().cloned())
        .and_then(|output| output.responses_file)
        .map(|file| file.file_name)
        .filter(|name| !name.is_empty())
}

async fn transcribe_with_batch(
    transcription: &Transcription,
    prompt: &str,
) -> Result<String, MyError> {
    if let (Some(chat_id), Some(draft_id)) = (transcription.chat_id, transcription.draft_id) {
        send_draft_progress(
            transcription.config.get_bot(),
            chat_id,
            draft_id,
            "🧩 <i>preparing_batch_chunks</i>".to_string(),
        )
        .await;
    }

    let chunks = match split_audio_to_chunks(
        transcription.data.as_ref(),
        &transcription.mime_type,
        AUDIO_CHUNK_SECONDS,
    ) {
        Ok(chunks) => chunks,
        Err(err) => {
            send_draft_debug(
                transcription.config.get_bot(),
                transcription.chat_id,
                transcription.draft_id,
                "chunk_split_failed",
                &err.to_string(),
            )
            .await;
            return Err(err);
        }
    };

    let mut file_manager = FileManager::new();
    file_manager.set_base_url(transcription.config.get_gemini_base_url());

    let mut uploaded_chunks: Vec<FileData> = Vec::new();
    for (idx, chunk) in chunks.iter().enumerate() {
        if let (Some(chat_id), Some(draft_id)) = (transcription.chat_id, transcription.draft_id) {
            send_draft_progress(
                transcription.config.get_bot(),
                chat_id,
                draft_id,
                format!("⬆️ <i>uploading_chunks</i> {}/{}", idx + 1, chunks.len()),
            )
            .await;
        }

        let file_data = match file_manager
            .add_file_from_bytes(
                &format!("audio_chunk_{}", idx),
                chunk.clone(),
                "audio/wav",
                Some(Duration::from_secs(FILE_UPLOAD_TIMEOUT_SECS)),
            )
            .await
        {
            Ok(file_data) => file_data,
            Err(err) => {
                send_draft_debug(
                    transcription.config.get_bot(),
                    transcription.chat_id,
                    transcription.draft_id,
                    "chunk_upload_failed",
                    &format!("chunk={} of {}: {}", idx + 1, chunks.len(), err),
                )
                .await;
                return Err(MyError::from(err));
            }
        };
        uploaded_chunks.push(file_data);
    }

    let mut jsonl = String::new();
    for (idx, file_data) in uploaded_chunks.iter().enumerate() {
        let request_line = serde_json::json!({
            "key": format!("{:05}", idx),
            "request": {
                "contents": [
                    {
                        "role": "user",
                        "parts": [
                            { "text": BATCH_REQUEST_PROMPT_TRANSCRIBE },
                            { "fileData": { "mimeType": file_data.mime_type, "fileUri": file_data.file_uri } }
                        ]
                    }
                ],
                "systemInstruction": {
                    "parts": [{ "text": prompt }]
                }
            }
        });

        jsonl.push_str(&serde_json::to_string(&request_line)?);
        jsonl.push('\n');
    }

    let request_file = match file_manager
        .add_file_from_bytes(
            "speech_batch_requests.jsonl",
            jsonl.into_bytes(),
            BATCH_REQUEST_MIME,
            Some(Duration::from_secs(FILE_UPLOAD_TIMEOUT_SECS)),
        )
        .await
    {
        Ok(file) => file,
        Err(err) => {
            send_draft_debug(
                transcription.config.get_bot(),
                transcription.chat_id,
                transcription.draft_id,
                "request_file_upload_failed",
                &err.to_string(),
            )
            .await;
            return Err(MyError::from(err));
        }
    };

    let request_file_name = match request_file
        .file_name
        .clone()
        .or_else(|| request_file.infer_file_name_from_uri())
    {
        Some(file_name) => file_name,
        None => {
            let err = anyhow!("Batch request upload did not return file name");
            send_draft_debug(
                transcription.config.get_bot(),
                transcription.chat_id,
                transcription.draft_id,
                "request_file_name_missing",
                &err.to_string(),
            )
            .await;
            return Err(err.into());
        }
    };

    let session = GemSession::Builder()
        .base_url(transcription.config.get_gemini_base_url())
        .model(Models::Custom(transcription.custom_model.clone()))
        .timeout(Some(Duration::from_secs(GENERATE_TIMEOUT_SECS)))
        .build();

    if let (Some(chat_id), Some(draft_id)) = (transcription.chat_id, transcription.draft_id) {
        send_draft_progress(
            transcription.config.get_bot(),
            chat_id,
            draft_id,
            "🛰️ <i>submitting_batch</i>".to_string(),
        )
        .await;
    }

    let mut submit_attempt = 0usize;
    let operation = loop {
        submit_attempt += 1;
        match session
            .create_generate_content_batch_from_file(
                &transcription.custom_model,
                &request_file_name,
                &format!("sr_batch_output_{}", Uuid::new_v4()),
                Some("speech_transcription"),
            )
            .await
        {
            Ok(operation) => break operation,
            Err(err) => {
                let err_text = redact_sensitive_text(&err.to_string());
                error!(
                    "Batch submit failed: model={} file={} chunks={} attempt={}/{} err={}",
                    transcription.custom_model,
                    request_file_name,
                    chunks.len(),
                    submit_attempt,
                    BATCH_SUBMIT_MAX_ATTEMPTS,
                    err_text
                );
                send_draft_debug(
                    transcription.config.get_bot(),
                    transcription.chat_id,
                    transcription.draft_id,
                    "batch_submit_failed",
                    &format!(
                        "attempt={}/{} {}",
                        submit_attempt, BATCH_SUBMIT_MAX_ATTEMPTS, err_text
                    ),
                )
                .await;

                if submit_attempt >= BATCH_SUBMIT_MAX_ATTEMPTS
                    || !is_retryable_error_text(&err_text)
                {
                    return Err(MyError::from(err));
                }

                let delay_secs = retry_backoff_seconds(submit_attempt);
                if let (Some(chat_id), Some(draft_id)) =
                    (transcription.chat_id, transcription.draft_id)
                {
                    send_draft_progress(
                        transcription.config.get_bot(),
                        chat_id,
                        draft_id,
                        format!(
                            "⏱️ <i>batch_submit_retry</i> {}/{} in {}s",
                            submit_attempt + 1,
                            BATCH_SUBMIT_MAX_ATTEMPTS,
                            delay_secs
                        ),
                    )
                    .await;
                }
                tokio::time::sleep(Duration::from_secs(delay_secs)).await;
            }
        }
    };

    let operation_name = operation.name.clone();
    if let (Some(chat_id), Some(draft_id)) = (transcription.chat_id, transcription.draft_id) {
        send_draft_progress(
            transcription.config.get_bot(),
            chat_id,
            draft_id,
            format!(
                "✅ <i>batch_submitted</i> <code>{}</code>",
                html::escape(&operation_name)
            ),
        )
        .await;
    }

    let _: () = transcription
        .config
        .get_redis_client()
        .set(
            &batch_pending_key(&operation_name),
            &request_file_name,
            PENDING_SLOT_TTL_SECS,
        )
        .await
        .unwrap_or_default();

    if let (Some(chat_id), Some(draft_id)) = (transcription.chat_id, transcription.draft_id) {
        send_draft_progress(
            transcription.config.get_bot(),
            chat_id,
            draft_id,
            "⏳ <i>waiting_batch_result</i>".to_string(),
        )
        .await;
    }

    let final_operation = match session
        .wait_until_done(
            &operation_name,
            Duration::from_secs(BATCH_POLL_INTERVAL_SECS),
            Duration::from_secs(BATCH_POLL_TIMEOUT_SECS),
        )
        .await
    {
        Ok(operation) => operation,
        Err(err) => {
            error!(
                "Batch polling failed for {}: {}",
                operation_name,
                redact_sensitive_text(&err.to_string())
            );
            send_draft_debug(
                transcription.config.get_bot(),
                transcription.chat_id,
                transcription.draft_id,
                "batch_poll_failed",
                &err.to_string(),
            )
            .await;
            return Err(MyError::from(err));
        }
    };

    if let Some(state) = final_operation.batch_state()
        && matches!(
            state,
            BatchState::JobStateFailed | BatchState::JobStateCancelled
        )
    {
        send_draft_debug(
            transcription.config.get_bot(),
            transcription.chat_id,
            transcription.draft_id,
            "batch_terminal_state",
            &format!("operation={} state={:?}", operation_name, state),
        )
        .await;
        return Err(anyhow!("Batch operation ended with terminal state: {:?}", state).into());
    }

    let response_file_name = match extract_response_file_name(&final_operation).await {
        Some(file_name) => file_name,
        None => {
            let err = anyhow!("Batch output file name was not returned");
            send_draft_debug(
                transcription.config.get_bot(),
                transcription.chat_id,
                transcription.draft_id,
                "response_file_name_missing",
                &err.to_string(),
            )
            .await;
            return Err(err.into());
        }
    };

    if let (Some(chat_id), Some(draft_id)) = (transcription.chat_id, transcription.draft_id) {
        send_draft_progress(
            transcription.config.get_bot(),
            chat_id,
            draft_id,
            "⬇️ <i>downloading_batch_results</i>".to_string(),
        )
        .await;
    }

    let result_content = match file_manager.download_file_string(&response_file_name).await {
        Ok(content) => content,
        Err(err) => {
            send_draft_debug(
                transcription.config.get_bot(),
                transcription.chat_id,
                transcription.draft_id,
                "batch_download_failed",
                &format!("file={} err={}", response_file_name, err),
            )
            .await;
            return Err(MyError::from(err));
        }
    };
    let parsed_lines = match parse_batch_jsonl_lines(&result_content) {
        Ok(lines) => lines,
        Err(err) => {
            send_draft_debug(
                transcription.config.get_bot(),
                transcription.chat_id,
                transcription.draft_id,
                "batch_jsonl_parse_failed",
                &err.to_string(),
            )
            .await;
            return Err(MyError::from(err));
        }
    };
    let grouped = group_batch_jsonl_by_key(parsed_lines);

    let mut parts = Vec::new();
    let mut line_error_count = 0usize;
    for (_, lines) in grouped {
        for line in lines {
            if let Some(error) = line.error {
                line_error_count += 1;
                error!("Batch line error [{}]: {}", line.key, error);
                continue;
            }

            if let Some(response) = line.response {
                let text = response.get_results().first().cloned().unwrap_or_default();
                if !text.is_empty() {
                    parts.push(text);
                }
            }
        }
    }

    if line_error_count > 0 {
        send_draft_debug(
            transcription.config.get_bot(),
            transcription.chat_id,
            transcription.draft_id,
            "batch_line_errors",
            &format!("{} JSONL entries failed", line_error_count),
        )
        .await;
    }

    if parts.is_empty() {
        send_draft_debug(
            transcription.config.get_bot(),
            transcription.chat_id,
            transcription.draft_id,
            "batch_empty_result",
            "Batch response did not contain transcription text",
        )
        .await;
        return Err(anyhow!("Batch response did not contain transcription text").into());
    }

    Ok(parts.join("\n\n"))
}

async fn summarize_audio_with_batch(
    mime_type: &str,
    data: &Bytes,
    config: &Config,
    action_type: &str,
    model: &str,
) -> Result<String, MyError> {
    let prompt = if action_type == "retell" {
        config.get_json_config().get_retell_prompt().to_owned()
    } else {
        config.get_json_config().get_summarize_prompt().to_owned()
    };

    let chunks = split_audio_to_chunks(data.as_ref(), mime_type, AUDIO_CHUNK_SECONDS)?;
    let mut file_manager = FileManager::new();
    file_manager.set_base_url(config.get_gemini_base_url());

    let mut uploaded_chunks: Vec<FileData> = Vec::new();
    for (idx, chunk) in chunks.iter().enumerate() {
        let file_data = file_manager
            .add_file_from_bytes(
                &format!("summary_chunk_{}", idx),
                chunk.clone(),
                "audio/wav",
                Some(Duration::from_secs(FILE_UPLOAD_TIMEOUT_SECS)),
            )
            .await
            .map_err(MyError::from)?;
        uploaded_chunks.push(file_data);
    }

    let mut jsonl = String::new();
    for (idx, file_data) in uploaded_chunks.iter().enumerate() {
        let line = serde_json::json!({
            "key": format!("{:05}", idx),
            "request": {
                "contents": [
                    {
                        "role": "user",
                        "parts": [
                            { "text": "Please process this audio chunk." },
                            { "fileData": { "mimeType": file_data.mime_type, "fileUri": file_data.file_uri } }
                        ]
                    }
                ],
                "systemInstruction": {
                    "parts": [{ "text": prompt }]
                }
            }
        });
        jsonl.push_str(&serde_json::to_string(&line)?);
        jsonl.push('\n');
    }

    let request_file = file_manager
        .add_file_from_bytes(
            "summary_batch_requests.jsonl",
            jsonl.into_bytes(),
            BATCH_REQUEST_MIME,
            Some(Duration::from_secs(FILE_UPLOAD_TIMEOUT_SECS)),
        )
        .await
        .map_err(MyError::from)?;

    let request_file_name = request_file
        .file_name
        .clone()
        .or_else(|| request_file.infer_file_name_from_uri())
        .ok_or_else(|| anyhow!("Batch request upload did not return file name"))?;

    let session = GemSession::Builder()
        .base_url(config.get_gemini_base_url())
        .model(Models::Custom(model.to_string()))
        .timeout(Some(Duration::from_secs(GENERATE_TIMEOUT_SECS)))
        .build();

    let operation = session
        .create_generate_content_batch_from_file(
            model,
            &request_file_name,
            &format!("sr_summary_batch_output_{}", Uuid::new_v4()),
            Some("speech_summary"),
        )
        .await
        .map_err(MyError::from)?;

    let final_operation = session
        .wait_until_done(
            &operation.name,
            Duration::from_secs(BATCH_POLL_INTERVAL_SECS),
            Duration::from_secs(BATCH_POLL_TIMEOUT_SECS),
        )
        .await
        .map_err(MyError::from)?;

    let response_file_name = extract_response_file_name(&final_operation)
        .await
        .ok_or_else(|| anyhow!("Batch output file name was not returned"))?;
    let content = file_manager
        .download_file_string(&response_file_name)
        .await
        .map_err(MyError::from)?;
    let lines = parse_batch_jsonl_lines(&content).map_err(MyError::from)?;
    let grouped = group_batch_jsonl_by_key(lines);
    let mut chunk_summaries = Vec::new();
    for (_, entries) in grouped {
        for entry in entries {
            if let Some(response) = entry.response {
                let text = response.get_results().first().cloned().unwrap_or_default();
                if !text.is_empty() {
                    chunk_summaries.push(text);
                }
            }
        }
    }

    if chunk_summaries.is_empty() {
        return Err(anyhow!("No summary chunks were produced by batch").into());
    }

    if chunk_summaries.len() == 1 {
        return Ok(chunk_summaries.remove(0));
    }

    let mut settings = Settings::new();
    settings.set_all_safety_settings(HarmBlockThreshold::BlockNone);
    settings.set_system_instruction(&prompt);
    let mut client = GemSession::Builder()
        .base_url(config.get_gemini_base_url())
        .model(Models::Custom(model.to_string()))
        .timeout(Some(Duration::from_secs(GENERATE_TIMEOUT_SECS)))
        .build();

    let merged = chunk_summaries.join("\n\n");
    let response = client
        .send_message(
            &format!(
                "Merge these chunk summaries into one coherent final output:\n\n{}",
                merged
            ),
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

impl Transcription {
    pub async fn to_text(&self, locale: &str) -> Vec<String> {
        let mut settings = Settings::new();
        settings.set_all_safety_settings(HarmBlockThreshold::BlockNone);

        let error_answer = t!("speech.error_transcription", locale = locale);
        let prompt = self.config.get_json_config().get_ai_prompt().to_owned();
        settings.set_system_instruction(&prompt);

        let mut batch_attempted = false;
        if should_use_batch(self.duration_seconds) {
            batch_attempted = true;
            match transcribe_with_batch(self, &prompt).await {
                Ok(text) => return split_text(&text, 4000),
                Err(err) => {
                    error!(
                        "Batch transcription failed, fallback to sync: {}",
                        redact_sensitive_text(&err.to_string())
                    );
                    send_draft_debug(
                        self.config.get_bot(),
                        self.chat_id,
                        self.draft_id,
                        "batch_to_sync_fallback",
                        &err.to_string(),
                    )
                    .await;
                }
            }
        }

        let mut file_manager = FileManager::new();
        file_manager.set_base_url(self.config.get_gemini_base_url());

        let file_data = match file_manager
            .add_file_from_bytes(
                "audio_transcription",
                self.data.to_vec(),
                &self.mime_type,
                Some(Duration::from_secs(FILE_UPLOAD_TIMEOUT_SECS)),
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
            .timeout(Some(Duration::from_secs(GENERATE_TIMEOUT_SECS)))
            .build();

        let mut attempts = 0usize;
        let mut last_error = String::new();
        while attempts < SYNC_TRANSCRIBE_MAX_ATTEMPTS {
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
                    if response.any_candidate_max_tokens() && !batch_attempted {
                        batch_attempted = true;
                        match transcribe_with_batch(self, &prompt).await {
                            Ok(text) => return split_text(&text, 4000),
                            Err(err) => {
                                error!(
                                    "Batch fallback after MAX_TOKENS failed: {}",
                                    redact_sensitive_text(&err.to_string())
                                );
                                last_error = format!("MAX_TOKENS; batch_fallback={}", err);
                                send_draft_debug(
                                    self.config.get_bot(),
                                    self.chat_id,
                                    self.draft_id,
                                    "max_tokens_batch_fallback_failed",
                                    &err.to_string(),
                                )
                                .await;
                            }
                        }
                    }
                    let full_text = response.get_results().first().cloned().unwrap_or_default();
                    if !full_text.is_empty() {
                        return split_text(&full_text, 4000);
                    }
                    attempts += 1;
                    info!("Received empty response, attempt {}", attempts);
                    if attempts < SYNC_TRANSCRIBE_MAX_ATTEMPTS {
                        let delay_secs = retry_backoff_seconds(attempts);
                        if let (Some(chat_id), Some(draft_id)) = (self.chat_id, self.draft_id) {
                            send_draft_progress(
                                self.config.get_bot(),
                                chat_id,
                                draft_id,
                                format!(
                                    "⏱️ <i>sync_retry_after_empty</i> {}/{} in {}s",
                                    attempts + 1,
                                    SYNC_TRANSCRIBE_MAX_ATTEMPTS,
                                    delay_secs
                                ),
                            )
                            .await;
                        }
                        tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                    }
                }
                Err(error) => {
                    attempts += 1;
                    if !batch_attempted
                        && matches!(
                            error.kind(),
                            GemErrorKind::Timeout | GemErrorKind::Connection
                        )
                    {
                        batch_attempted = true;
                        match transcribe_with_batch(self, &prompt).await {
                            Ok(text) => return split_text(&text, 4000),
                            Err(err) => {
                                error!(
                                    "Batch fallback after sync error failed: {}",
                                    redact_sensitive_text(&err.to_string())
                                );
                                send_draft_debug(
                                    self.config.get_bot(),
                                    self.chat_id,
                                    self.draft_id,
                                    "sync_error_batch_fallback_failed",
                                    &err.to_string(),
                                )
                                .await;
                            }
                        }
                    }

                    let error_string = redact_sensitive_text(&error.to_string());
                    error!(
                        "Transcription error (attempt {}/{}): {:?}",
                        attempts, SYNC_TRANSCRIBE_MAX_ATTEMPTS, error_string
                    );

                    let safe_error = compact_debug_text(&error_string, DEBUG_MESSAGE_MAX_CHARS);

                    if safe_error == last_error {
                        if attempts >= SYNC_TRANSCRIBE_MAX_ATTEMPTS {
                            break;
                        }
                    } else {
                        last_error = safe_error;
                    }

                    if attempts >= SYNC_TRANSCRIBE_MAX_ATTEMPTS {
                        break;
                    }

                    if !is_retryable_error_text(&error_string) {
                        break;
                    }

                    let delay_secs = retry_backoff_seconds(attempts);
                    if let (Some(chat_id), Some(draft_id)) = (self.chat_id, self.draft_id) {
                        send_draft_progress(
                            self.config.get_bot(),
                            chat_id,
                            draft_id,
                            format!(
                                "⏱️ <i>sync_retry_wait</i> {}/{} in {}s",
                                attempts + 1,
                                SYNC_TRANSCRIBE_MAX_ATTEMPTS,
                                delay_secs
                            ),
                        )
                        .await;
                    }
                    tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                }
            }
        }
        vec![error_answer.to_string() + "\n\n" + &last_error]
    }
}
