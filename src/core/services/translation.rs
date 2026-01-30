use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Engine {
    #[default]
    Google,
    DeepL,
    Bing,
    Yandex,
    LibreTranslate,
    MyMemory,
}

impl Engine {
    pub fn all() -> &'static [Engine] {
        &[
            Engine::Google,
            Engine::DeepL,
            Engine::Bing,
            Engine::Yandex,
            Engine::LibreTranslate,
            Engine::MyMemory,
        ]
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Engine::Google => "Google Translate",
            Engine::DeepL => "DeepL",
            Engine::Bing => "Bing Microsoft",
            Engine::Yandex => "Yandex Translate",
            Engine::LibreTranslate => "LibreTranslate",
            Engine::MyMemory => "MyMemory",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Engine::Google => "google",
            Engine::DeepL => "deepl",
            Engine::Bing => "bing",
            Engine::Yandex => "yandex",
            Engine::LibreTranslate => "libretranslate",
            Engine::MyMemory => "mymemory",
        }
    }

    pub fn supports_auto(&self) -> bool {
        matches!(self, Engine::Google | Engine::Bing | Engine::Yandex)
    }
}

pub const SUPPORTED_LANGUAGES: &[(&str, &str)] = &[
    ("uk", "🇺🇦 Українська"),
    ("en", "🇬🇧 English"),
    ("us", "🇺🇸 English (US)"),
    ("ru", "🇷🇺 Русский"),
    ("de", "🇩🇪 Deutsch"),
    ("fr", "🇫🇷 Français"),
    ("es", "🇪🇸 Español"),
    ("it", "🇮🇹 Italiano"),
    ("zh", "🇨🇳 中文"),
    ("ja", "🇯🇵 日本語"),
    ("ko", "🇰🇷 한국어"),
    ("pl", "🇵🇱 Polski"),
    ("ar", "🇸🇦 العربية"),
    ("pt", "🇵🇹 Português"),
    ("tr", "🇹🇷 Türkçe"),
    ("nl", "🇳🇱 Nederlands"),
    ("sv", "🇸🇪 Svenska"),
    ("no", "🇳🇴 Norsk"),
    ("da", "🇩🇰 Dansk"),
    ("fi", "🇫🇮 Suomi"),
    ("el", "🇬🇷 Ελληνικά"),
    ("he", "🇮🇱 עברית"),
    ("hi", "🇮🇳 हिन्दी"),
    ("id", "🇮🇩 Indonesia"),
    ("vi", "🇻🇳 Tiếng Việt"),
    ("th", "🇹🇭 ภาษาไทย"),
    ("cs", "🇨🇿 Čeština"),
    ("hu", "🇭🇺 Magyar"),
    ("ro", "🇷🇴 Română"),
    ("bg", "🇧🇬 Български"),
    ("sr", "🇷🇸 Српски"),
    ("hr", "🇭🇷 Hrvatski"),
    ("sk", "🇸🇰 Slovenčina"),
    ("sl", "🇸🇮 Slovenščina"),
    ("lt", "🇱🇹 Lietuvių"),
    ("lv", "🇱🇻 Latviešu"),
    ("et", "🇪🇪 Eesti"),
];
pub const LANGUAGES_PER_PAGE: usize = 6;

pub fn normalize_language_code(lang: &str) -> String {
    match lang.to_lowercase().as_str() {
        "ua" | "ukrainian" | "украинский" | "uk" => "uk".to_string(),
        "ru" | "russian" | "русский" => "ru".to_string(),
        "en" | "english" | "английский" => "en".to_string(),
        "de" | "german" | "немецкий" => "de".to_string(),
        "fr" | "french" | "французский" => "fr".to_string(),
        "es" | "spanish" | "испанский" => "es".to_string(),
        "it" | "italian" | "итальянский" => "it".to_string(),
        "zh" | "chinese" | "китайский" => "zh".to_string(),
        "ja" | "japanese" | "японский" => "ja".to_string(),
        _ => lang.to_lowercase(),
    }
}

#[derive(Debug, Deserialize)]
pub struct MozhiResponse {
    #[serde(rename = "translated-text")]
    pub translated_text: String,

    pub detected: Option<String>,

    pub engine: Option<String>,
}

#[derive(Clone)]
pub struct MozhiClient {
    base_url: String,
    http: Client,
}

impl MozhiClient {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            http: Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent("Fulturate/6.6.6 (rust) (+https://github.com/weever1337/fulturate-rs)")
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn request(
        &self,
        text: impl Into<String>,
        target_lang: impl Into<String>,
    ) -> TranslationBuilder {
        TranslationBuilder {
            client: self.clone(),
            text: text.into(),
            target_lang: target_lang.into(),
            source_lang: None,
            engine: Engine::Google, // Default engine
        }
    }

    pub async fn detect_language(&self, text: &str) -> Result<String, anyhow::Error> {
        let params = [
            ("engine", "google"),
            ("from", "auto"),
            ("to", "en"),
            ("text", text),
        ];

        let resp = self
            .http
            .get(format!("{}/translate", self.base_url))
            .query(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(anyhow::anyhow!(
                "Detection failed: status {}",
                resp.status()
            ));
        }

        let data: MozhiResponse = resp.json().await?;
        Ok(data.detected.unwrap_or_else(|| "en".to_string()))
    }
}

pub struct TranslationBuilder {
    client: MozhiClient,
    text: String,
    target_lang: String,
    source_lang: Option<String>,
    engine: Engine,
}

impl TranslationBuilder {
    pub fn source(mut self, lang: impl Into<String>) -> Self {
        self.source_lang = Some(lang.into());
        self
    }

    pub fn engine(mut self, engine: Engine) -> Self {
        self.engine = engine;
        self
    }

    pub async fn send(self) -> Result<String, anyhow::Error> {
        let final_source = if self.source_lang.as_deref().unwrap_or("auto") == "auto"
            && !self.engine.supports_auto()
        {
            self.client
                .detect_language(&self.text)
                .await
                .unwrap_or("auto".to_string())
        } else {
            self.source_lang.unwrap_or("auto".to_string())
        };

        let params = [
            ("engine", self.engine.as_str()),
            ("from", &final_source),
            ("to", &self.target_lang),
            ("text", &self.text),
        ];

        let resp = self
            .client
            .http
            .get(format!("{}/translate", self.client.base_url))
            .query(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(anyhow::anyhow!(
                "Translation failed: status {}",
                resp.status()
            ));
        }

        let data: MozhiResponse = resp.json().await?;
        Ok(data.translated_text)
    }
}
