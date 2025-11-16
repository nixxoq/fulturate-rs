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
use teloxide::{
    prelude::*,
    types::{
        ChatId, ChosenInlineResult, FileId, InlineQuery, InlineQueryResult,
        InlineQueryResultArticle, InlineQueryResultPhoto, InputFile, InputMedia, InputMediaVideo,
        InputMessageContent, InputMessageContentText,
    },
};
use tokio::fs;

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

mod video_metadata {
    use super::{MyError, TempGuard};
    use serde::Deserialize;
    use std::path::{Path, PathBuf};
    use tokio::process::Command;

    const USER_AGENT: &str = "Fulturate/6.6.6 (rust) (+https://github.com/weever1337/fulturate-rs)";

    #[derive(Debug, Clone)]
    pub struct VideoMetadata {
        pub duration: u32,
        pub width: u32,
        pub height: u32,
        pub thumbnail: String,
    }

    #[derive(Deserialize)]
    struct YtdlpOutput {
        duration: Option<f64>,
        width: Option<u32>,
        height: Option<u32>,
        thumbnail: Option<String>,
    }

    pub async fn get_from_url(url: &str) -> Result<VideoMetadata, MyError> {
        let ytdlp_output = Command::new("yt-dlp")
            .args(["--dump-json", url])
            .output()
            .await?;

        if !ytdlp_output.status.success() {
            let stderr = String::from_utf8_lossy(&ytdlp_output.stderr);
            log::error!("yt-dlp failed for URL '{}'. Stderr: {}", url, stderr);
            return Err("Failed to execute yt-dlp".into());
        }

        let metadata: YtdlpOutput = serde_json::from_slice(&ytdlp_output.stdout)?;
        Ok(VideoMetadata {
            duration: metadata.duration.unwrap_or(0.0) as u32,
            width: metadata.width.unwrap_or(0),
            height: metadata.height.unwrap_or(0),
            thumbnail: metadata.thumbnail.unwrap_or_default(),
        })
    }

    pub async fn download_thumbnail(
        url: &str,
        output_dir: &Path,
        file_hash: &str,
    ) -> Result<(PathBuf, TempGuard), MyError> {
        let thumb_path = output_dir.join(format!("{}_thumb.jpg", file_hash));
        let response = reqwest::get(url).await?.bytes().await?;
        tokio::fs::write(&thumb_path, &response).await?;
        let guard = TempGuard {
            path: thumb_path.clone(),
        };
        Ok((thumb_path, guard))
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
        const MAX_DURATION_SECONDS: u32 = 10 * 60; // todo: move this value to other place
        match video_metadata::get_from_url(url).await {
            Ok(meta) if meta.duration > MAX_DURATION_SECONDS => {
                let minutes = MAX_DURATION_SECONDS / 60;
                let error_article = InlineQueryResultArticle::new(
                    "error_duration",
                    "Видео слишком длинное",
                    InputMessageContent::Text(InputMessageContentText::new(format!(
                        "❌ Видео дольше {} минут и не может быть обработано.",
                        minutes
                    ))),
                )
                .description(format!("Максимальная длительность: {} минут", minutes));
                bot.answer_inline_query(q.id, vec![error_article.into()])
                    .await?;
                return Ok(());
            }
            Err(e) => {
                log::warn!("Could not get video metadata with yt-dlp: {}", e);
            }
            _ => {}
        }

        let settings = Settings::get_module_settings::<CobaltSettings>(&owner, "cobalt").await?;
        let cobalt_client = config.get_cobalt_client();
        match resolve_download_url(url, &settings, cobalt_client).await {
            Ok(Some(download_result)) => {
                let cache_entry = CobaltCache::Pending(download_result.clone());
                redis.set(&cache_key, &cache_entry, 24 * 60 * 60).await?;
                build_results_from_media(url, download_result, &url_hash, user_id)
            }
            _ => {
                vec![]
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

    bot.edit_message_text_inline(&inline_message_id, "⏳ Обрабатываю видео...")
        .await?;

    let redis = config.get_redis_client();
    let cache_key = format!("cobalt_cache:{}", url_hash);
    let cached_data = redis.get::<CobaltCache>(&cache_key).await?;

    let (file_id, original_url) = match cached_data {
        Some(CobaltCache::Ready {
            file_id,
            original_url,
            ..
        }) => (file_id, original_url),
        Some(CobaltCache::Pending(DownloadResult::Video {
            url: video_url,
            original_url,
            ..
        })) => {
            let temp_dir = PathBuf::from("./temp_videos");
            fs::create_dir_all(&temp_dir).await?;

            let meta = video_metadata::get_from_url(&original_url).await?;
            let (thumb_path, _thumb_guard) =
                video_metadata::download_thumbnail(&video_url, &temp_dir, url_hash).await?;

            let log_channel_id = ChatId(config.get_log_chat_id().parse().unwrap());
            let thumb_msg = bot
                .send_photo(log_channel_id, InputFile::file(thumb_path))
                .await?;
            let thumb_file_id = thumb_msg
                .photo()
                .and_then(|p| p.last())
                .map(|s| s.file.id.clone())
                .ok_or("Failed to get thumb file_id")?;
            bot.delete_message(log_channel_id, thumb_msg.id).await.ok();

            bot.edit_message_text_inline(&inline_message_id, "⏳ Загружаю видео...")
                .await?;

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

            let client = config.get_cobalt_client();
            let video_path = client
                .download_and_save(&cobalt_req, url_hash, &temp_dir.to_string_lossy())
                .await?;
            let _video_guard = TempGuard {
                path: video_path.clone(),
            };

            let video_msg = bot
                .send_video(log_channel_id, InputFile::file(&video_path))
                .duration(meta.duration)
                .width(meta.width)
                .height(meta.height)
                .thumbnail(InputFile::file_id(thumb_file_id.clone()))
                .await?;

            let video = video_msg
                .video()
                .ok_or("Failed to get video from message")?;
            let file_id = video.file.id.clone();

            let ready_cache = CobaltCache::Ready {
                file_id: file_id.to_string(),
                original_url: original_url.clone(),
                duration: meta.duration,
                width: meta.width,
                height: meta.height,
                thumb_file_id: thumb_file_id.to_string(),
            };
            redis
                .set(&cache_key, &ready_cache, 365 * 24 * 60 * 60)
                .await?;

            (file_id.to_string(), original_url)
        }
        _ => {
            bot.edit_message_text_inline(inline_message_id, "❌ Ошибка: видео не найдено в кэше.")
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
