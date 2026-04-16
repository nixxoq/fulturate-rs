/// This module contains constants and types related to the Gemini API.
///
/// Gem-rs is a Rust library that serves as a wrapper around the Gemini API,
/// providing support for streaming. This library is designed to facilitate
/// interaction with the Gemini API, making it easier to integrate its
/// functionalities into Rust projects.
use serde::{Deserialize, Serialize};

/// Base URL for generating content using the Gemini API.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// Enum representing different Gemini API models.
///
/// This enum includes various versions of Gemini models, including experimental
/// and stable versions. The default model is set to `Gemini15Pro`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum Models {
    /// Experimental Gemini 1.5 Pro model (version 0827)
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemini-1.5-pro-exp-0827")]
    Gemini15ProExp0827,

    /// Experimental Gemini 1.5 Flash model (version 0827)
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemini-1.5-flash-exp-0827")]
    Gemini15FlashExp0827,

    /// Experimental Gemini 1.5 Flash 8B model (version 0827)
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemini-1.5-flash-8b-exp-0827")]
    Gemini15Flash8bExp0827,

    /// Gemini 1.5 Pro model
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemini-1.5-pro")]
    Gemini15Pro,

    /// Default Gemini 2 Flash Experimental model
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemini-2.0-flash-exp")]
    Gemini2FlashExp,

    /// Gemini 2 Experimental model
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemini-exp-1206")]
    Gemini2Exp1206,

    /// Gemini 1.5 Flash model
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemini-1.5-flash")]
    Gemini15Flash,

    /// Gemini 1.0 Pro model
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemini-1.0-pro")]
    Gemini10Pro,

    /// Gemma 2 2B IT model
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemma-2-2b-it")]
    Gemma2_2bIt,

    /// Gemma 2 9B IT model
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemma-2-9b-it")]
    Gemma2_9bIt,

    /// Gemma 2 27B IT model
    #[deprecated = "Use custom model instead, if you need to use older models"]
    #[serde(rename = "gemma-2-27b-it")]
    Gemma2_27bIt,

    #[serde(rename = "gemini-2.0-flash")]
    Gemini2Flash,

    #[serde(rename = "gemini-2.0-flash-lite")]
    Gemini2FlashLite,

    #[serde(rename = "gemini-2.5-pro")]
    Gemini25Pro,

    #[serde(rename = "gemini-2.5-flash")]
    Gemini25Flash,

    #[default]
    #[serde(rename = "gemini-2.5-flash-lite-latest")]
    Gemini25FlashLiteLatest,

    #[serde(rename = "gemini-2.5-flash-image")]
    Gemini25FlashImagePreview,

    #[serde(rename = "gemini-3.0-pro-preview")]
    Gemini30ProPreview,

    #[serde(rename = "gemini-3.0-flash-preview")]
    Gemini30FlashPreview,

    #[serde(rename = "gemma-3-1b-it")]
    Gemma3_1b,

    #[serde(rename = "gemma-3-4b-it")]
    Gemma3_4b,

    #[serde(rename = "gemma-3-12b-it")]
    Gemma3_12b,

    #[serde(rename = "gemma-3-27b-it")]
    Gemma3_27b,

    /// Custom model
    Custom(String),
}

impl ToString for Models {
    fn to_string(&self) -> String {
        match self {
            Models::Custom(model) => model.to_string().replace("\"", ""),
            _ => serde_json::to_string(self).unwrap().replace("\"", ""),
        }
    }
}
