use crate::{
    bot::{
        keyboards::cobalt::{make_photo_pagination_keyboard, make_single_url_keyboard},
        modules::{Owner, cobalt::CobaltSettings},
    },
    core::{
        config::Config,
        db::schemas::settings::Settings,
        services::cobalt::{CobaltCache, DownloadResult, resolve_download_url},
    },
    errors::MyError,
};
use ccobalt::model::request::{DownloadRequest, FilenameStyle};
use once_cell::sync::Lazy;
use regex::Regex;
use std::{path::PathBuf, sync::Arc};
use serde::Deserialize;
use teloxide::{
    prelude::*,
    types::{
        ChatId, ChosenInlineResult, FileId, InlineQuery, InlineQueryResult,
        InlineQueryResultArticle, InlineQueryResultPhoto, InputFile, InputMedia, InputMediaVideo,
        InputMessageContent, InputMessageContentText,
    },
};
use tokio::fs;
use tokio::process::Command;

static URL_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(https?)://[^\s/$.?#].[^\s]*$").unwrap());

struct TempGuard {
    path: PathBuf,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            log::error!("Failed to delete temp file {:?}: {}", self.path, e);
        }
    }
}

#[derive(Deserialize)]
struct FfprobeOutput {
    streams: Vec<Stream>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: String,
    width: Option<i32>,
    height: Option<i32>,
    #[serde(with = "duration_parser")]
    duration: Option<f64>,
}

mod duration_parser {
    use serde::{self, Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: Option<&str> = Option::deserialize(deserializer)?;
        if let Some(s) = s {
            s.parse::<f64>().map(Some).map_err(serde::de::Error::custom)
        } else {
            Ok(None)
        }
    }
}

pub async fn is_query_url(inline_query: InlineQuery) -> bool {
    if !URL_REGEX.is_match(&inline_query.query) {
        return false;
    };

    let owner = Owner {
        id: inline_query.from.id.to_string(),
        r#type: "user".to_string(),
    };

    match Settings::get_module_settings::<CobaltSettings>(&owner, "cobalt").await {
        Ok(settings) => settings.enabled,
        Err(_) => false,
    }
}

fn build_results_from_media(
    original_url: &str,
    media: DownloadResult,
    url_hash: &str,
    user_id: u64,
) -> Vec<InlineQueryResult> {
    match media {
        DownloadResult::Video { .. } => {
            let url_kb = make_single_url_keyboard(original_url);
            let result = InlineQueryResultArticle::new(
                format!("cobalt_video:{}", url_hash),
                "Скачать видео",
                InputMessageContent::Text(InputMessageContentText::new(
                    "Нажмите, чтобы отправить видео",
                )),
            )
            .reply_markup(url_kb);

            vec![result.into()]
        }
        DownloadResult::Photos { urls, .. } => {
            let total = urls.len();
            urls.into_iter()
                .enumerate()
                .filter_map(|(i, url_str)| {
                    if let (Ok(photo_url), Ok(thumb_url)) = (url_str.parse(), url_str.parse()) {
                        let result_id = format!("{}_{}", url_hash, i);

                        let keyboard = if total > 1 {
                            make_photo_pagination_keyboard(
                                url_hash,
                                i,
                                total,
                                user_id,
                                original_url,
                            )
                        } else {
                            make_single_url_keyboard(original_url)
                        };

                        let photo_result =
                            InlineQueryResultPhoto::new(result_id, photo_url, thumb_url)
                                .reply_markup(keyboard);

                        Some(photo_result.into())
                    } else {
                        None
                    }
                })
                .collect()
        }
    }
}

pub async fn handle_cobalt_inline(
    bot: Bot,
    q: InlineQuery,
    config: Arc<Config>,
) -> Result<(), MyError> {
    let url = q.query.trim();
    if !URL_REGEX.is_match(url) {
        return Ok(());
    }

    let user_id = q.from.id.0;
    let owner = Owner {
        id: q.from.id.to_string(),
        r#type: "user".to_string(),
    };

    let url_hash_digest = md5::compute(url);
    let url_hash = format!("{:x}", url_hash_digest);
    let cache_key = format!("cobalt_cache:{}", url_hash);

    let redis = config.get_redis_client();

    let results = if let Ok(Some(cached_entry)) = redis.get::<CobaltCache>(&cache_key).await {
        let download_result = match cached_entry {
            CobaltCache::Pending(dr) => dr,
            CobaltCache::Ready { original_url, .. } => DownloadResult::Video {
                url: original_url.clone(),
                original_url,
                filename: None,
            },
        };
        let original_url_str = match &download_result {
            DownloadResult::Video { original_url, .. } => original_url.clone(),
            DownloadResult::Photos { original_url, .. } => original_url.clone(),
        };
        build_results_from_media(&original_url_str, download_result, &url_hash, user_id)
    } else {
        let settings = Settings::get_module_settings::<CobaltSettings>(&owner, "cobalt").await?;
        let cobalt_client = config.get_cobalt_client();
        let result = resolve_download_url(url, &settings, cobalt_client).await;

        match result {
            Ok(Some(download_result)) => {
                let cache_entry = CobaltCache::Pending(download_result.clone());
                if let Err(e) = redis.set(&cache_key, &cache_entry, 24 * 60 * 60).await {
                    log::error!("Failed to cache cobalt result: {}", e);
                }
                build_results_from_media(url, download_result, &url_hash, user_id)
            }
            _ => {
                let error_article = InlineQueryResultArticle::new(
                    "error",
                    "Error",
                    InputMessageContent::Text(InputMessageContentText::new(
                        "Failed to process link. Media not found or an error occurred.",
                    )),
                )
                .description("Could not fetch media. Please try again later.");
                vec![error_article.into()]
            }
        }
    };
    bot.answer_inline_query(q.id, results).cache_time(0).await?;
    Ok(())
}

pub async fn handle_inline_video(
    bot: Bot,
    chosen: ChosenInlineResult,
    config: Arc<Config>,
) -> Result<(), MyError> {
    let Some(inline_message_id) = chosen.inline_message_id else {
        return Ok(());
    };
    let Some(url_hash) = chosen.result_id.strip_prefix("cobalt_video:") else {
        return Ok(());
    };

    bot.edit_message_text_inline(&inline_message_id, "⏳ Загружаю видео...")
        .await?;

    let redis = config.get_redis_client();
    let cache_key = format!("cobalt_cache:{}", url_hash);

    let cached_data = redis.get::<CobaltCache>(&cache_key).await?;

    let (file_id, original_url) = match cached_data {
        Some(CobaltCache::Ready {
            file_id,
            original_url,
        }) => (file_id, original_url),
        Some(CobaltCache::Pending(DownloadResult::Video { original_url, .. })) => {
            let owner = Owner {
                id: chosen.from.id.to_string(),
                r#type: "user".to_string(),
            };
            let settings =
                Settings::get_module_settings::<CobaltSettings>(&owner, "cobalt").await?;

            let cobalt_req = DownloadRequest {
                url: original_url.clone(),
                filename_style: Some(FilenameStyle::Pretty),
                video_quality: Some(match settings.video_quality {
                    crate::core::services::cobalt::VideoQuality::Q720 => {
                        ccobalt::model::request::VideoQuality::Q720
                    }
                    crate::core::services::cobalt::VideoQuality::Q1080 => {
                        ccobalt::model::request::VideoQuality::Q1080
                    }
                    crate::core::services::cobalt::VideoQuality::Q1440 => {
                        ccobalt::model::request::VideoQuality::Q1440
                    }
                    crate::core::services::cobalt::VideoQuality::Max => {
                        ccobalt::model::request::VideoQuality::Max
                    }
                }),
                ..Default::default()
            };

            let temp = "./temp_videos";
            fs::create_dir_all(&temp).await?;

            let client = config.get_cobalt_client();
            let video_path = client.download_and_save(&cobalt_req, url_hash, temp).await?;
            let _video_guard = TempGuard { path: video_path.clone() };

            let ffprobe_output = Command::new("ffprobe")
                .args([
                    "-v", "quiet",
                    "-print_format", "json",
                    "-show_streams",
                    video_path.to_str().unwrap(),
                ])
                .output().await?;

            let metadata: FfprobeOutput = serde_json::from_slice(&ffprobe_output.stdout)?;
            let video_stream = metadata.streams.iter().find(|s| s.codec_type == "video");
            let (duration, width, height) = if let Some(stream) = video_stream {
                (
                    stream.duration.unwrap_or(0.0) as u32,
                    stream.width.unwrap_or(0),
                    stream.height.unwrap_or(0),
                )
            } else {
                (0, 0, 0)
            };

            let thumb_path = video_path.with_extension("jpg");
            Command::new("ffmpeg")
                .args([
                    "-i", video_path.to_str().unwrap(),
                    "-ss", "00:00:01.000",
                    "-vframes", "1",
                    thumb_path.to_str().unwrap(),
                ])
                .status().await?;

            let _thumb_guard = TempGuard { path: thumb_path.clone() };

            let log_channel = ChatId(config.get_log_chat_id().parse().unwrap());
            let msg = bot
                .send_video(log_channel, InputFile::file(&video_path))
                .thumbnail(InputFile::file(&thumb_path))
                .duration(duration)
                .width(width as u32)
                .height(height as u32)
                .await?;

            let video = msg.video().ok_or("Failed to get video from message")?;
            let file_id = video.file.id.clone();

            let ready_cache = CobaltCache::Ready {
                file_id: file_id.clone().to_string(),
                original_url: original_url.clone(),
            };
            let ttl_one_year = 365 * 24 * 60 * 60;
            if let Err(e) = redis.set(&cache_key, &ready_cache, ttl_one_year).await {
                log::error!("Failed to save permanent file_id to cache: {}", e);
            }

            (file_id.to_string(), original_url)
        }
        _ => {
            bot.edit_message_text_inline(
                inline_message_id,
                "❌ Ошибка: видео не найдено в кэше или кеш поврежден.",
            )
            .await?;
            return Ok(());
        }
    };

    let media = InputMedia::Video(InputMediaVideo::new(InputFile::file_id(FileId::from(
        file_id,
    ))));
    let url_kb = make_single_url_keyboard(&original_url);

    if let Err(e) = bot
        .edit_message_media_inline(&inline_message_id, media)
        .reply_markup(url_kb)
        .await
    {
        log::error!("Failed to send video with file_id: {:?}", e);
        bot.edit_message_text_inline(inline_message_id, "❌ Ошибка: не удалось отправить видео.")
            .await?;
    }

    Ok(())
}
