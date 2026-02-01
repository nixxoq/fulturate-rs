use crate::{
    bot::modules::cobalt::CobaltSettings,
    core::{
        config::Config,
        metrics::{API_LATENCY, INCOMING_UPDATES, MODULE_USAGE},
    },
    errors::MyError,
};
use ccobalt::model::{
    request::{DownloadRequest, FilenameStyle},
    response::DownloadResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum VideoQuality {
    Q720,
    Q1080,
    Q1440,
    Max,
}

impl VideoQuality {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoQuality::Q720 => "720",
            VideoQuality::Q1080 => "1080",
            VideoQuality::Q1440 => "1440",
            VideoQuality::Max => "max",
        }
    }

    pub fn parse_quality(s: &str) -> Self {
        match s {
            "1080" => VideoQuality::Q1080,
            "1440" => VideoQuality::Q1440,
            "max" => VideoQuality::Max,
            _ => VideoQuality::Q720,
        }
    }
}

#[derive(Default, Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum AudioQuality {
    K128,
    #[default]
    K256,
    K320,
}

impl AudioQuality {
    pub fn as_str(&self) -> &'static str {
        match self {
            AudioQuality::K128 => "128",
            AudioQuality::K256 => "256",
            AudioQuality::K320 => "320",
        }
    }

    pub fn parse_quality(s: &str) -> Self {
        match s {
            "128" => AudioQuality::K128,
            "256" => AudioQuality::K256,
            "320" => AudioQuality::K320,
            _ => AudioQuality::K256,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum DownloadResult {
    Video {
        url: String,
        original_url: String,
        filename: Option<String>,
    },
    Photos {
        urls: Vec<String>,
        original_url: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum CobaltCache {
    Pending(DownloadResult),
    Ready {
        file_id: String,
        original_url: String,
        duration: u32,
        width: u32,
        height: u32,
        thumb_file_id: String,
    },
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CobaltDirectoryResponse {
    pub data: Vec<InstanceData>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct InstanceData {
    pub api: String,
    pub tests: HashMap<String, TestResult>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct TestResult {
    pub friendly: Option<String>,
    pub status: bool,
}

pub async fn get_cobalt_status(config: &Config) -> Result<Option<InstanceData>, MyError> {
    let cache = config.get_redis_client();
    let cache_key = "cobalt_status_cache";

    if let Ok(Some(cached)) = cache.get::<String>(cache_key).await
        && let Ok(data) = serde_json::from_str::<InstanceData>(&cached)
    {
        return Ok(Some(data));
    }

    let client = reqwest::Client::builder()
        .user_agent("Fulturate/6.6.6 (rust) (+https://github.com/weever1337/fulturate-rs)")
        .build()?;

    let response = client
        .get("https://cobalt.directory/api/tests")
        .send()
        .await?
        .json::<CobaltDirectoryResponse>()
        .await?;

    let base_api = config
        .get_cobalt_base_api()
        .replace("https://", "")
        .replace("http://", "")
        .trim_end_matches('/')
        .to_string();

    let instance = response.data.into_iter().find(|i| i.api == base_api);

    if let Some(ref found) = instance {
        let encoded = serde_json::to_string(found)?;
        let _: () = cache.set(cache_key, &encoded, 60).await?;
    }

    Ok(instance)
}

pub fn get_total_updates() -> u64 {
    INCOMING_UPDATES.get()
}

pub async fn resolve_download_url(
    url: &str,
    settings: &CobaltSettings,
    client: &ccobalt::Client,
) -> Result<Option<DownloadResult>, MyError> {
    MODULE_USAGE.with_label_values(&["cobalt", "resolve"]).inc();

    let cobalt_req = DownloadRequest {
        url: url.to_string(),
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
    let timer = API_LATENCY.with_label_values(&["cobalt"]).start_timer();
    let response = client.resolve_download(&cobalt_req).await?;
    timer.observe_duration();
    match response {
        DownloadResponse::Error { error } => {
            log::error!("Cobalt API error: {:?}", error);
            Err(error.into())
        }
        DownloadResponse::Picker { picker, .. } => {
            let photo_urls: Vec<String> = picker
                .iter()
                .filter(|item| item.kind == "photo")
                .map(|item| item.url.clone())
                .collect();
            if !photo_urls.is_empty() {
                return Ok(Some(DownloadResult::Photos {
                    urls: photo_urls,
                    original_url: url.to_string(),
                }));
            }
            if let Some(video_item) = picker.iter().find(|item| item.kind == "video") {
                return Ok(Some(DownloadResult::Video {
                    url: video_item.url.clone(),
                    original_url: url.to_string(),
                    filename: None,
                }));
            }
            Ok(None)
        }
        DownloadResponse::Tunnel {
            url: c_url,
            filename,
        }
        | DownloadResponse::Redirect {
            url: c_url,
            filename,
        } => {
            const PHOTO_EXTENSIONS: &[&str] = &[".jpg", ".jpeg", ".png", ".gif", ".webp"];
            let is_photo = PHOTO_EXTENSIONS
                .iter()
                .any(|ext| filename.to_lowercase().ends_with(ext));

            if is_photo {
                Ok(Some(DownloadResult::Photos {
                    urls: vec![c_url.clone()],
                    original_url: url.to_string(),
                }))
            } else {
                Ok(Some(DownloadResult::Video {
                    url: c_url,
                    original_url: url.to_string(),
                    filename: Some(filename),
                }))
            }
        }
        _ => Ok(response
            .get_download_url()
            .map(|c_url| DownloadResult::Video {
                url: c_url,
                original_url: url.to_string(),
                filename: None,
            })),
    }
}
