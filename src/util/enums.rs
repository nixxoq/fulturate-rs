use crate::t;
use ccobalt::model::error::CobaltError;
use teloxide::{
    types::{InlineQueryResultArticle, InputMessageContent, InputMessageContentText},
    utils::command::BotCommands,
};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    #[command(description = "Start command")]
    Start(String),
    #[command(description = "Help command")]
    Help,
    #[command(description = "Speech recognition", alias = "sr")]
    SpeechRecognition,
    #[command(description = "Translate", alias = "tr")]
    Translate(String),
    #[command(description = "Bot settings")]
    Settings,

    #[command(description = "Admin Panel", hide)]
    Admin,
    #[command(description = "Broadcast message (reply only)", hide)]
    Broadcast,
}

pub struct AudioStruct {
    pub mime_type: String,
    pub file_id: String,
    pub file_unique_id: String,
}

pub enum CobaltErrorType {
    RateLimit { seconds: u32 },
    Restricted,
    Unknown,
}

impl CobaltErrorType {
    pub(crate) fn from_error(err: &anyhow::Error) -> Self {
        if let Some(cobalt_error) = err.downcast_ref::<CobaltError>() {
            return match cobalt_error.code.as_str() {
                "error.api.rate_exceeded" => {
                    let seconds = cobalt_error
                        .context
                        .as_ref()
                        .and_then(|c| c.limit)
                        .unwrap_or(60);
                    Self::RateLimit { seconds }
                }
                code if matches!(
                    code,
                    "error.api.content.video.unavailable"
                        | "error.api.content.video.age"
                        | "error.api.content.video.private"
                        | "error.api.content.video.region"
                ) =>
                {
                    Self::Restricted
                }
                _ => Self::Unknown,
            };
        }

        let err_str = err.to_string();
        if err_str.contains("Sign in to confirm your age")
            || err_str.contains("error.api.content.video.unavailable")
        {
            return Self::Restricted;
        }

        Self::Unknown
    }

    pub(crate) fn into_article(self, locale: &str) -> InlineQueryResultArticle {
        let (id, title, description) = match self {
            Self::RateLimit { seconds } => (
                "error_ratelimit",
                t!("modules.cobalt.error_ratelimit_title", locale = locale),
                t!(
                    "modules.cobalt.error_ratelimit",
                    locale = locale,
                    seconds = seconds
                ),
            ),
            Self::Restricted => (
                "error_restricted",
                t!("modules.cobalt.error_restricted_title", locale = locale),
                t!("modules.cobalt.error_restricted", locale = locale),
            ),
            Self::Unknown => (
                "error_processing",
                t!("modules.cobalt.error_processing_title", locale = locale),
                t!("modules.cobalt.error_processing", locale = locale),
            ),
        };

        InlineQueryResultArticle::new(
            id,
            title,
            InputMessageContent::Text(InputMessageContentText::new(description.clone())),
        )
        .description(description)
    }
}
