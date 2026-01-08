mod json;

use crate::core::{
    config::json::{JsonConfig, read_json_config},
    db::redis::RedisCache,
    services::currency::converter::CurrencyConverter,
};
use dotenv::dotenv;
use gem_rs::api::DEFAULT_BASE_URL;
use log::error;
use redis::Client as RedisClient;
use std::sync::Arc;
use teloxide::prelude::*;

#[derive(Clone)]
pub struct Config {
    bot: Bot,
    cobalt_client: ccobalt::Client,
    #[allow(dead_code)]
    owners: Vec<String>,
    log_chat_id: String,
    error_chat_thread_id: String,
    archive_chat_id: String,
    version: String,
    json_config: JsonConfig,
    currency_converter: Arc<CurrencyConverter>,
    mongodb_url: String,
    redis_client: RedisCache,
    telegram_api: String,
    gemini_base_url: String,
    trash_channel_id: String,
}

impl Config {
    pub async fn new() -> Self {
        dotenv().ok();
        let version = env!("CARGO_PKG_VERSION").to_string();

        let Ok(bot_token) = std::env::var("BOT_TOKEN") else {
            error!("Expected BOT_TOKEN env var");
            std::process::exit(1);
        };

        let Ok(cobalt_base_api) = std::env::var("COBALT_BASE_API") else {
            error!("COBALT_BASE_API expected");
            std::process::exit(1);
        };

        let Ok(telegram_api_url) = std::env::var("TELEGRAM_API_URL") else {
            error!("TELEGRAM_API_URL expected");
            std::process::exit(1);
        };

        let url = reqwest::Url::parse(&telegram_api_url).unwrap();
        let bot = Bot::new(bot_token).set_api_url(url);

        let mut cobalt_client = ccobalt::Client::builder()
            .base_url(cobalt_base_api)
            // .base_url("https://cobalt-backend.canine.tools/")
            // .base_url("http://127.0.0.1:9000")
            // .base_url("https://nixxo.local/")
            // .no_api_key(true)
            // .api_key(cobalt_api_key)
            .user_agent(
                "Fulturate/6.6.6 (rust) (+https://github.com/weever1337/fulturate-rs)".to_string(),
            );

        match std::env::var("COBALT_API_KEY") {
            Ok(key) if !key.is_empty() => {
                cobalt_client = cobalt_client.api_key(key);
            }
            _ => {
                cobalt_client = cobalt_client.no_api_key(true);
            }
        }

        let cobalt_client = cobalt_client.build().unwrap_or_else(|_err| {
            error!("Failed to build cobalt client");
            std::process::exit(1);
        });

        let owners: Vec<String> = std::env::var("OWNERS")
            .unwrap_or_else(|_| {
                error!("OWNERS expected");
                std::process::exit(1)
            })
            .split(',')
            .filter_map(|id| id.trim().parse().ok())
            .collect();

        let Ok(log_chat_id) = std::env::var("LOG_CHAT_ID") else {
            error!("LOG_CHAT_ID expected");
            std::process::exit(1);
        };
        let error_chat_thread_id: String = std::env::var("ERROR_CHAT_THREAD_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.to_string());

        let archive_chat_id: String = std::env::var("ARCHIVE_CHAT_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.to_string());

        let Ok(json_config) = read_json_config("config.json") else {
            error!("Unable to read config.json");
            std::process::exit(1);
        };
        let currency_converter = Arc::new(CurrencyConverter::new().unwrap());
        let Ok(mongodb_url) = std::env::var("MONGODB_URL") else {
            error!("MONGODB_URL expected");
            std::process::exit(1);
        };

        let Ok(redis_url) = std::env::var("REDIS_URL") else {
            error!("REDIS_URL expected");
            std::process::exit(1);
        };

        let Ok(redis_client) = RedisClient::open(redis_url.to_owned()) else {
            error!("Failed to open Redis client");
            std::process::exit(1);
        };
        let redis_client = RedisCache::new(redis_client);

        let gemini_base_url =
            std::env::var("GEMINI_BASE_URL").unwrap_or(DEFAULT_BASE_URL.to_string());

        let Ok(trash_channel_id) = std::env::var("TRASH_CHANNEL_ID") else {
            error!("TRASH_CHANNEL_ID expected");
            std::process::exit(1);
        };

        Config {
            bot,
            cobalt_client,
            owners,
            log_chat_id,
            error_chat_thread_id,
            archive_chat_id,
            version,
            json_config,
            currency_converter,
            mongodb_url,
            redis_client,
            telegram_api: telegram_api_url,
            gemini_base_url,
            trash_channel_id,
        }
    }

    pub fn get_bot(&self) -> &Bot {
        &self.bot
    }

    pub fn get_cobalt_client(&self) -> &ccobalt::Client {
        &self.cobalt_client
    }

    pub fn get_version(&self) -> &str {
        &self.version
    }

    #[allow(dead_code)]
    pub fn is_id_in_owners(&self, id: String) -> bool {
        self.owners.contains(&id)
    }

    pub fn get_log_chat_id(&self) -> &str {
        &self.log_chat_id
    }

    pub fn get_trash_channel_id(&self) -> &str {
        &self.trash_channel_id
    }

    pub fn get_error_chat_thread_id(&self) -> &str {
        &self.error_chat_thread_id
    }

    pub fn get_archive_chat_id(&self) -> &str {
        &self.archive_chat_id
    }

    pub fn get_json_config(&self) -> &JsonConfig {
        &self.json_config
    }

    pub fn get_currency_converter(&self) -> &CurrencyConverter {
        &self.currency_converter
    }

    pub fn get_mongodb_url(&self) -> &str {
        &self.mongodb_url
    }

    pub fn get_redis_client(&self) -> &RedisCache {
        &self.redis_client
    }

    pub fn get_telegram_api(&self) -> &str {
        &self.telegram_api
    }

    pub fn get_gemini_base_url(&self) -> &str {
        &self.gemini_base_url
    }
}
