use crate::bot::modules::Owner;
use crate::core::config::Config;
use crate::core::db::schemas::settings::Settings;
use lazy_static::lazy_static;
use log::{debug, error, info, warn};
use mongodb::bson::doc;
use oximod::ModelTrait;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::RwLock;
use teloxide::types::{Chat, User};

lazy_static! {
    static ref TRANSLATIONS: RwLock<HashMap<String, Value>> = RwLock::new(HashMap::new());
    static ref VAR_REGEX: Regex = Regex::new(r"%\{(\w+)\}").unwrap();
}

pub const DEFAULT_LOCALE: &str = "en";

const REPO_OWNER: &str = "Fulturate";
const REPO_NAME: &str = "locales";
const REPO_PATH: &str = "locales";
const LOCALES_DIR: &str = "locales";

#[derive(Deserialize, Debug)]
struct GitHubFile {
    name: String,
    download_url: Option<String>,
}

pub async fn check_and_update_locales() -> bool {
    let path = Path::new(LOCALES_DIR);
    if !path.exists() {
        let _ = fs::create_dir(path);
    }

    let client = reqwest::Client::builder()
        .user_agent("FulturateBot")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default();

    let api_url = if REPO_PATH.is_empty() {
        format!(
            "https://api.github.com/repos/{}/{}/contents",
            REPO_OWNER, REPO_NAME
        )
    } else {
        format!(
            "https://api.github.com/repos/{}/{}/contents/{}",
            REPO_OWNER, REPO_NAME, REPO_PATH
        )
    };

    let files_list: Vec<GitHubFile> = match client.get(&api_url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                warn!("[LOCALE] Failed to get file list: HTTP {}", resp.status());
                return false;
            }
            match resp.json::<Vec<GitHubFile>>().await {
                Ok(files) => files,
                Err(e) => {
                    error!("[LOCALE] Failed to parse GitHub response: {}", e);
                    return false;
                }
            }
        }
        Err(e) => {
            error!("[LOCALE] Network error getting file list: {}", e);
            return false;
        }
    };

    let mut any_updated = false;

    for file in files_list {
        if !file.name.ends_with(".json") {
            continue;
        }

        let Some(download_url) = file.download_url else {
            continue;
        };

        let file_path = path.join(&file.name);

        match client.get(&download_url).send().await {
            Ok(resp) => {
                if !resp.status().is_success() {
                    warn!("[LOCALE] Failed to fetch {}: {}", file.name, resp.status());
                    continue;
                }

                match resp.bytes().await {
                    Ok(remote_bytes) => {
                        let remote_hash = md5::compute(&remote_bytes);

                        let local_hash = if file_path.exists() {
                            match fs::read(&file_path) {
                                Ok(local_bytes) => Some(md5::compute(local_bytes)),
                                Err(_) => None,
                            }
                        } else {
                            None
                        };

                        if local_hash.is_none() || local_hash.unwrap() != remote_hash {
                            if let Err(e) = fs::write(&file_path, &remote_bytes) {
                                error!("[LOCALE] Failed to write {}: {}", file.name, e);
                            } else {
                                info!("[LOCALE] Updated: {}", file.name);
                                any_updated = true;
                            }
                        } else {
                            debug!("[LOCALE] {} is up to date", file.name);
                        }
                    }
                    Err(e) => error!("[LOCALE] Failed to read bytes for {}: {}", file.name, e),
                }
            }
            Err(e) => error!("[LOCALE] Network error for {}: {}", file.name, e),
        }
    }

    any_updated
}

pub fn load_locales() {
    let path = Path::new("locales");
    if !path.exists() {
        let _ = fs::create_dir(path);
        warn!("[LOCALE] Folder 'locales' not found (yet). Waiting for download.");
        return;
    }

    let mut store = TRANSLATIONS.write().unwrap();
    store.clear();

    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json")
                && let Some(file_stem) = path.file_stem().and_then(|s| s.to_str())
            {
                let lang_code = file_stem.to_string();
                if let Ok(content) = fs::read_to_string(&path)
                    && let Ok(json) = serde_json::from_str::<Value>(&content)
                {
                    store.insert(lang_code, json);
                }
            }
        }
    }
    info!("[LOCALE] Locales loaded into memory.");
}

pub fn get_available_locales() -> Vec<String> {
    let store = TRANSLATIONS.read().unwrap();
    let mut keys: Vec<String> = store.keys().cloned().collect();
    keys.sort();
    keys
}

pub fn translate(key: &str, locale: &str, args: Option<HashMap<&str, String>>) -> String {
    let store = TRANSLATIONS.read().unwrap();

    let find_val = |lang: &str, k: &str| -> Option<String> {
        let mut current = store.get(lang)?;
        for part in k.split('.') {
            current = current.get(part)?;
        }
        current.as_str().map(|s| s.to_string())
    };

    let raw_text = find_val(locale, key).or_else(|| find_val(DEFAULT_LOCALE, key));

    if let Some(text) = raw_text {
        if let Some(args) = args {
            return VAR_REGEX
                .replace_all(&text, |caps: &regex::Captures| {
                    args.get(&caps[1])
                        .cloned()
                        .unwrap_or_else(|| caps.get(0).unwrap().as_str().to_string())
                })
                .to_string();
        }
        return text;
    }

    key.to_string()
}

pub fn is_supported(lang: &str) -> bool {
    let store = TRANSLATIONS.read().unwrap();
    store.contains_key(lang)
}

pub fn normalize_lang_code(code: Option<&str>) -> String {
    let code = code
        .and_then(|c| c.split('-').next())
        .unwrap_or(DEFAULT_LOCALE);
    if is_supported(code) {
        code.to_string()
    } else {
        DEFAULT_LOCALE.to_string()
    }
}

pub async fn get_locale_by_owner(owner_id: &str, owner_type: &str, config: &Config) -> String {
    let redis_key = format!("locale:{}:{}", owner_type, owner_id);
    let redis = config.get_redis_client();

    if let Ok(Some(cached_lang)) = redis.get::<String>(&redis_key).await {
        return cached_lang;
    }

    let owner = Owner {
        id: owner_id.to_string(),
        r#type: owner_type.to_string(),
    };

    let db_lang = match Settings::get_or_create(&owner).await {
        Ok(settings) => settings.language,
        Err(_) => String::new(),
    };

    let final_lang = if is_supported(&db_lang) {
        db_lang
    } else {
        DEFAULT_LOCALE.to_string()
    };

    let _ = redis.set(&redis_key, &final_lang, 3600 * 24).await;
    final_lang
}

pub async fn get_chat_locale(chat: &Chat, config: &Config) -> String {
    if chat.is_private() {
        get_locale_by_owner(&chat.id.to_string(), "user", config).await
    } else {
        get_locale_by_owner(&chat.id.to_string(), "group", config).await
    }
}

pub async fn get_user_locale(user: &User, config: &Config) -> String {
    let lang = get_locale_by_owner(&user.id.0.to_string(), "user", config).await;

    if lang == DEFAULT_LOCALE && !is_supported(&lang) {
        normalize_lang_code(user.language_code.as_deref())
    } else {
        lang
    }
}

pub async fn get_locale_by_id(user_id: u64, config: &Config) -> String {
    get_locale_by_owner(&user_id.to_string(), "user", config).await
}

pub async fn set_locale(
    owner_id: &str,
    owner_type: &str,
    new_lang: &str,
    config: &Config,
) -> Result<(), crate::errors::MyError> {
    if !is_supported(new_lang) {
        return Ok(());
    }

    Settings::update_one(
        doc! { "owner_id": owner_id, "owner_type": owner_type },
        doc! { "$set": { "language": new_lang } },
    )
    .await?;

    let redis_key = format!("locale:{}:{}", owner_type, owner_id);
    config
        .get_redis_client()
        .set(&redis_key, &new_lang.to_string(), 3600 * 24)
        .await?;

    Ok(())
}

#[macro_export]
macro_rules! t {
    ($key:expr, locale = $locale:expr) => {
        $crate::util::i18n::translate($key, $locale, None)
    };
    ($key:expr, locale = $locale:expr, $($arg:ident = $val:expr),* $(,)?) => {
        {
            let mut args = std::collections::HashMap::new();
            $(
                args.insert(stringify!($arg), $val.to_string());
            )*
            $crate::util::i18n::translate($key, $locale, Some(args))
        }
    };
}
