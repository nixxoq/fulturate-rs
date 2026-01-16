use crate::{
    bot::{
        keyboards::cobalt::{make_photo_pagination_keyboard, make_single_url_keyboard},
        modules::{Owner, cobalt::CobaltSettings},
    },
    core::{
        config::Config,
        db::schemas::settings::Settings,
        services::cobalt::{
            AudioQuality, CobaltCache, DownloadResult, VideoQuality, resolve_download_url,
        },
    },
    errors::MyError,
    t,
    util::{MAX_DURATION_SECONDS, i18n::get_user_locale},
};
use anyhow::anyhow;
use ccobalt::model::request::{DownloadRequest, FilenameStyle};
use once_cell::sync::Lazy;
use regex::Regex;
use std::{path::PathBuf, sync::Arc};
use teloxide::{
    prelude::*,
    types::{
        ChatId, ChosenInlineResult, FileId, InlineQuery, InlineQueryResult,
        InlineQueryResultArticle, InlineQueryResultPhoto, InputFile, InputMedia, InputMediaAudio,
        InputMediaVideo, InputMessageContent, InputMessageContentText,
    },
};
use tokio::fs;

pub static URL_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(https?)://[^\s/$.?#].\S*$").unwrap());

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
    use anyhow::Context;
    use serde::Deserialize;
    use std::path::{Path, PathBuf};
    use tokio::process::Command;

    #[derive(Debug, Clone)]
    pub struct VideoMetadata {
        pub duration: u32,
        pub width: u32,
        pub height: u32,
        pub thumbnail_url: Option<String>,
        pub title: Option<String>,
    }

    #[derive(Deserialize)]
    struct YtDlpOutput {
        duration: Option<f64>,
        width: Option<u32>,
        height: Option<u32>,
        thumbnail: Option<String>,
        title: Option<String>,
    }

    pub async fn get_from_url(url: &str) -> Result<VideoMetadata, MyError> {
        let ytdlp_output = Command::new("yt-dlp")
            .args(["--js-runtimes", "node"])
            .args(["--dump-json", url])
            .output()
            .await?;

        if !ytdlp_output.status.success() {
            let stderr = String::from_utf8_lossy(&ytdlp_output.stderr);
            log::error!("yt-dlp failed for URL '{}'. Stderr: {}", url, stderr);
            return Err(anyhow::anyhow!("Failed to execute yt-dlp"));
        }

        let metadata: YtDlpOutput = serde_json::from_slice(&ytdlp_output.stdout)?;
        Ok(VideoMetadata {
            duration: metadata.duration.unwrap_or(0.0) as u32,
            width: metadata.width.unwrap_or(0),
            height: metadata.height.unwrap_or(0),
            thumbnail_url: metadata.thumbnail,
            title: metadata.title,
        })
    }

    pub async fn download_thumbnail(
        url: &str,
        output_dir: &Path,
        file_hash: &str,
    ) -> Result<(PathBuf, TempGuard), MyError> {
        let thumb_path = output_dir.join(format!("{}_thumb.jpg", file_hash));
        let response = reqwest::get(url).await?.bytes().await?;

        let path_clone = thumb_path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), MyError> {
            let img =
                image::load_from_memory(&response).context("Failed to load image from memory")?;
            let scaled = img.resize(320, 320, image::imageops::FilterType::Lanczos3);

            scaled
                .save_with_format(&path_clone, image::ImageFormat::Jpeg)
                .context("Failed to save scaled thumbnail")?;

            Ok(())
        })
        .await??;

        let guard = TempGuard {
            path: thumb_path.clone(),
        };
        Ok((thumb_path, guard))
    }
}

pub async fn is_query_url(inline_query: InlineQuery) -> bool {
    if !URL_REGEX.is_match(&inline_query.query.trim()) {
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
    locale: &str,
) -> Vec<InlineQueryResult> {
    match media {
        DownloadResult::Video { .. } => {
            let url_kb = make_single_url_keyboard(original_url);
            let result = InlineQueryResultArticle::new(
                format!("cobalt_video:{}", url_hash),
                t!("modules.cobalt.inline_download", locale = locale),
                InputMessageContent::Text(InputMessageContentText::new(t!(
                    "modules.cobalt.inline_send",
                    locale = locale
                ))),
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

    let locale = get_user_locale(&q.from, &config).await;

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
        build_results_from_media(
            &original_url_str,
            download_result,
            &url_hash,
            user_id,
            &locale,
        )
    } else {
        match video_metadata::get_from_url(url).await {
            Ok(meta) if meta.duration > MAX_DURATION_SECONDS => {
                let minutes = MAX_DURATION_SECONDS / 60;

                let error_article = InlineQueryResultArticle::new(
                    "error_duration",
                    t!("modules.cobalt.error_duration_title", locale = &locale),
                    InputMessageContent::Text(InputMessageContentText::new(t!(
                        "modules.cobalt.error_too_long",
                        locale = &locale,
                        minutes = minutes
                    ))),
                )
                .description(t!(
                    "modules.cobalt.error_too_long",
                    locale = &locale,
                    minutes = minutes
                ));
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
                build_results_from_media(url, download_result, &url_hash, user_id, &locale)
            }
            _ => {
                let error_article = InlineQueryResultArticle::new(
                    "error_processing",
                    t!("modules.cobalt.error_processing_title", locale = &locale),
                    InputMessageContent::Text(InputMessageContentText::new(t!(
                        "modules.cobalt.error_processing",
                        locale = &locale
                    ))),
                );
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

    let locale = get_user_locale(&chosen.from, &config).await;

    bot.edit_message_text_inline(
        &inline_message_id,
        t!("modules.cobalt.processing", locale = &locale),
    )
    .await?;

    let redis = config.get_redis_client();
    let cache_key = format!("cobalt_cache:{}", url_hash);
    let cached_data = redis.get::<CobaltCache>(&cache_key).await?;

    let (file_id, original_url, thumb_file_id, is_audio) = match cached_data {
        Some(CobaltCache::Ready {
            file_id,
            original_url,
            thumb_file_id,
            width,
            height,
            ..
        }) => {
            let is_audio = width == 0 && height == 0;
            (file_id, original_url, thumb_file_id, is_audio)
        }
        Some(CobaltCache::Pending(DownloadResult::Video {
            url: _video_url,
            original_url,
            ..
        })) => {
            let temp_dir = PathBuf::from("./temp_videos");
            fs::create_dir_all(&temp_dir).await?;

            let meta = video_metadata::get_from_url(&original_url).await?;
            let thumb_data = if let Some(thumb_url) = &meta.thumbnail_url {
                video_metadata::download_thumbnail(thumb_url, &temp_dir, url_hash)
                    .await
                    .ok()
            } else {
                None
            };

            bot.edit_message_text_inline(
                &inline_message_id,
                t!("modules.cobalt.uploading", locale = &locale),
            )
            .await?;

            let trash_channel = ChatId(config.get_trash_channel_id().parse()?);

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
                    VideoQuality::Q720 => ccobalt::model::request::VideoQuality::Q720,
                    VideoQuality::Q1080 => ccobalt::model::request::VideoQuality::Q1080,
                    VideoQuality::Q1440 => ccobalt::model::request::VideoQuality::Q1440,
                    VideoQuality::Max => ccobalt::model::request::VideoQuality::Max,
                }),
                audio_bitrate: Some(match settings.audio_quality {
                    AudioQuality::K128 => ccobalt::model::request::AudioBitrate::Kbps128,
                    AudioQuality::K256 => ccobalt::model::request::AudioBitrate::Kbps256,
                    AudioQuality::K320 => ccobalt::model::request::AudioBitrate::Kbps320,
                }),
                ..Default::default()
            };

            let client = config.get_cobalt_client();

            let response = client.resolve_download(&cobalt_req).await?;
            let download_url = response
                .get_download_url()
                .ok_or_else(|| anyhow!("Cobalt returned no download URL (Picker or Error?)"))?;

            let http_client = reqwest::Client::builder()
                .user_agent(
                    "Fulturate/6.6.6 (rust) (+https://github.com/weever1337/fulturate-rs)"
                        .to_string(),
                )
                .timeout(std::time::Duration::from_secs(600))
                .build()?;

            let (mut attempts, mut success, mut last_error, mut is_audio, mut path) = (
                0,
                false,
                String::new(),
                false,
                temp_dir.join(format!("{}.mp4", url_hash)),
            );

            while attempts < 3 {
                attempts += 1;
                match http_client.get(&download_url).send().await {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            if let Some(cd) =
                                resp.headers().get(reqwest::header::CONTENT_DISPOSITION)
                            {
                                let cd_str = cd.to_str().unwrap_or("").to_lowercase();
                                if cd_str.contains(".mp3")
                                    || cd_str.contains(".ogg")
                                    || cd_str.contains(".wav")
                                    || cd_str.contains(".m4a")
                                {
                                    is_audio = true;
                                    path = temp_dir.join(format!("{}.mp3", url_hash));
                                }
                            }

                            if !is_audio
                                && let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE)
                            {
                                let ct_str = ct.to_str().unwrap_or("");
                                if ct_str.contains("audio/") {
                                    is_audio = true;
                                    path = temp_dir.join(format!("{}.mp3", url_hash));
                                }
                            }

                            let content = resp.bytes().await?;
                            if !content.is_empty() {
                                fs::write(&path, content).await?;
                                success = true;
                                break;
                            } else {
                                last_error = "File is empty".into();
                            }
                        } else {
                            last_error = format!("HTTP {}", resp.status());
                            if !resp.status().is_server_error() {
                                break;
                            }
                        }
                    }
                    Err(e) => last_error = e.to_string(),
                }
                if !success && attempts < 3 {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }

            if !success {
                bot.edit_message_text_inline(
                    &inline_message_id,
                    format!("❌ Download Failed: {}", last_error),
                )
                .await?;
                return Ok(());
            }

            let _guard = TempGuard { path: path.clone() };

            let upload_client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3600))
                .connect_timeout(std::time::Duration::from_secs(60))
                .build()?;

            let uploader = Bot::with_client(bot.token(), upload_client).set_api_url(bot.api_url());

            let sent_msg = if is_audio {
                let mut audio_msg = uploader.send_audio(trash_channel, InputFile::file(&path));

                if let Some(title) = &meta.title {
                    audio_msg = audio_msg.title(title);
                }
                audio_msg = audio_msg.duration(meta.duration);
                if let Some((path, _)) = &thumb_data {
                    audio_msg = audio_msg.thumbnail(InputFile::file(path));
                }

                audio_msg.await?
            } else {
                let mut video_msg = uploader
                    .send_video(trash_channel, InputFile::file(&path))
                    .duration(meta.duration)
                    .width(meta.width)
                    .height(meta.height);

                if let Some((path, _)) = &thumb_data {
                    video_msg = video_msg.thumbnail(InputFile::file(path));
                }

                video_msg.await?
            };

            let (file_id, thumb_id) = if let Some(audio) = sent_msg.audio() {
                (
                    audio.file.id.clone(),
                    audio
                        .thumbnail
                        .as_ref()
                        .map(|t| t.file.id.clone())
                        .unwrap_or_default(),
                )
            } else if let Some(video) = sent_msg.video() {
                (
                    video.file.id.clone(),
                    video
                        .thumbnail
                        .as_ref()
                        .map(|t| t.file.id.clone())
                        .unwrap_or_default(),
                )
            } else if let Some(doc) = sent_msg.document() {
                (
                    doc.file.id.clone(),
                    doc.thumbnail
                        .as_ref()
                        .map(|t| t.file.id.clone())
                        .unwrap_or_default(),
                )
            } else {
                return Err(anyhow!("Sent message does not contain supported media"));
            };

            let ready_cache = CobaltCache::Ready {
                file_id: file_id.clone().to_string(),
                original_url: original_url.clone(),
                duration: meta.duration,
                width: if is_audio { 0 } else { meta.width },
                height: if is_audio { 0 } else { meta.height },
                thumb_file_id: thumb_id.clone().to_string(),
            };
            redis
                .set(&cache_key, &ready_cache, 365 * 24 * 60 * 60)
                .await?;

            (file_id.0, original_url, thumb_id.0, is_audio)
        }
        _ => {
            bot.edit_message_text_inline(
                inline_message_id,
                t!("modules.cobalt.cache_error", locale = &locale),
            )
            .await?;
            return Ok(());
        }
    };

    let url_kb = make_single_url_keyboard(&original_url);

    if is_audio {
        let mut media = InputMediaAudio::new(InputFile::file_id(FileId::from(file_id)));
        if !thumb_file_id.is_empty() {
            media = media.thumbnail(InputFile::file_id(FileId::from(thumb_file_id)));
        }
        if let Err(e) = bot
            .edit_message_media_inline(&inline_message_id, InputMedia::Audio(media))
            .reply_markup(url_kb)
            .await
        {
            log::error!("Failed to edit inline audio: {:?}", e);
            bot.edit_message_text_inline(
                inline_message_id,
                t!("modules.cobalt.send_error", locale = &locale),
            )
            .await?;
        }
    } else {
        let mut media = InputMediaVideo::new(InputFile::file_id(FileId::from(file_id)));
        if !thumb_file_id.is_empty() {
            media = media.thumbnail(InputFile::file_id(FileId::from(thumb_file_id)));
        }
        if let Err(e) = bot
            .edit_message_media_inline(&inline_message_id, InputMedia::Video(media))
            .reply_markup(url_kb)
            .await
        {
            log::error!("Failed to edit inline video: {:?}", e);
            bot.edit_message_text_inline(
                inline_message_id,
                t!("modules.cobalt.send_error", locale = &locale),
            )
            .await?;
        }
    }

    Ok(())
}
