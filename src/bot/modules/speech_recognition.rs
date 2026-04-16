use crate::{
    bot::modules::{Module, ModuleSettings, Owner},
    core::{config::Config, db::schemas::settings::Settings},
    errors::MyError,
    t,
    util::i18n::get_locale_by_owner,
};
use serde::{Deserialize, Serialize};
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum SpeechModel {
    Gemini25FlashLite,
    Gemini25Flash,
    Gemini25Pro,
    Gemini3Flash,
}

impl SpeechModel {
    pub fn display_name(&self) -> &str {
        match self {
            Self::Gemini25FlashLite => "Gemini 2.5 Flash-Lite",
            Self::Gemini25Flash => "Gemini 2.5 Flash",
            Self::Gemini3Flash => "Gemini 3 Flash",
            Self::Gemini25Pro => "Gemini 2.5 Pro",
        }
    }

    pub fn api_key(&self) -> &str {
        match self {
            Self::Gemini25FlashLite => "gemini-2.5-flash-lite",
            Self::Gemini25Flash => "gemini-2.5-flash",
            Self::Gemini3Flash => "gemini-3-flash-preview",
            Self::Gemini25Pro => "gemini-2.5-pro",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "Gemini 2.5 Pro" => Self::Gemini25Pro,
            "Gemini 3 Flash" => Self::Gemini3Flash,
            "Gemini 2.5 Flash-Lite" => Self::Gemini25FlashLite,
            _ => Self::Gemini25Flash,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SpeechRecognitionSettings {
    pub enabled: bool,
    pub transcription_model: SpeechModel,
    pub summary_model: SpeechModel,
    pub enable_voice: bool,
    pub enable_video_note: bool,
    pub enable_audio: bool,
}

impl Default for SpeechRecognitionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            transcription_model: SpeechModel::Gemini25Flash,
            summary_model: SpeechModel::Gemini25Flash,
            enable_voice: true,
            enable_video_note: true,
            enable_audio: false,
        }
    }
}

impl ModuleSettings for SpeechRecognitionSettings {}

module! {
    struct SpeechRecognitionModule;
    settings = SpeechRecognitionSettings;
    key = "speech";
    name = "Speech Recognition";
    desc = "Модуль для обработки, расшифровки и пересказа голосовых сообщений.";
    designed_for = "all";

    impl {
        async fn get_settings_ui(
            &self,
            owner: &Owner,
            cid: u64,
        ) -> Result<(String, InlineKeyboardMarkup), MyError> {
            self.render_main_menu(owner, cid).await
        }

        async fn handle_callback(
            &self,
            bot: Bot,
            q: &CallbackQuery,
            owner: &Owner,
            data: &str,
            cid: u64,
        ) -> Result<(), MyError> {
            let Some(msg) = q.message.as_ref().and_then(|m| m.regular_message()) else { return Ok(()); };
            let parts: Vec<_> = data.split(':').collect();

            if parts.is_empty() { return Ok(()); }

            let mut s: SpeechRecognitionSettings = Settings::get_module_settings(owner, self.key()).await?;
            let mut save = false;
            let mut view = "main";

            match parts[0] {
                "toggle_module" => { s.enabled = !s.enabled; save = true; }
                "toggle_voice" => { s.enable_voice = !s.enable_voice; save = true; }
                "toggle_video" => { s.enable_video_note = !s.enable_video_note; save = true; }
                "toggle_audio" => { s.enable_audio = !s.enable_audio; save = true; }
                "menu" if parts.len() >= 2 => {
                    view = parts[1];
                    let (text, keyboard) = match view {
                        "models" => self.render_models_menu(owner, cid).await?,
                        _ => self.render_main_menu(owner, cid).await?,
                    };
                    bot.edit_message_text(msg.chat.id, msg.id, text)
                        .reply_markup(keyboard)
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                    return Ok(());
                }
                "set_trans_model" if parts.len() >= 2 => {
                    s.transcription_model = SpeechModel::from_str(parts[1]);
                    save = true;
                    view = "models";
                }
                "set_sum_model" if parts.len() >= 2 => {
                    s.summary_model = SpeechModel::from_str(parts[1]);
                    save = true;
                    view = "models";
                }
                _ => {}
            }

            if save {
                Settings::update_module_settings(owner, self.key(), s).await?;
                let (text, keyboard) = match view {
                    "models" => self.render_models_menu(owner, cid).await?,
                    _ => self.render_main_menu(owner, cid).await?,
                };
                bot.edit_message_text(msg.chat.id, msg.id, text)
                    .reply_markup(keyboard)
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
            } else {
                bot.answer_callback_query(q.clone().id).await?;
            }

            Ok(())
        }
    }
}

impl SpeechRecognitionModule {
    async fn render_main_menu(
        &self,
        owner: &Owner,
        cid: u64,
    ) -> Result<(String, InlineKeyboardMarkup), MyError> {
        let s: SpeechRecognitionSettings = Settings::get_module_settings(owner, self.key()).await?;
        let config = Config::new().await;
        let locale = get_locale_by_owner(&owner.id, &owner.r#type, &config).await;

        let module_name = t!("modules.speech.name", locale = &locale);
        let module_desc = t!("modules.speech.desc", locale = &locale);
        let status_key = if s.enabled {
            "modules.status_on"
        } else {
            "modules.status_off"
        };
        let status_text = t!(status_key, locale = &locale);

        let header = t!(
            "modules.status_header",
            locale = &locale,
            name = module_name,
            desc = module_desc,
            status = status_text
        );

        let trans_lbl = t!("modules.speech.transcription", locale = &locale);
        let sum_lbl = t!("modules.speech.summary", locale = &locale);
        let msg_types_lbl = t!("modules.speech.msg_types", locale = &locale);

        let text = format!(
            "{}\n\n<b>{}:</b> {}\n<b>{}:</b> {}\n\n{}",
            header,
            trans_lbl,
            s.transcription_model.display_name(),
            sum_lbl,
            s.summary_model.display_name(),
            msg_types_lbl
        );

        let toggle = |lbl: String, val: bool, cb: &str| {
            InlineKeyboardButton::callback(
                format!("{} {}", if val { "✅" } else { "❌" }, lbl),
                format!("{}:settings:{}:{}", self.key(), cb, cid),
            )
        };

        let toggle_mod_key = if s.enabled {
            "settings.toggle_off"
        } else {
            "settings.toggle_on"
        };
        let toggle_mod_btn = InlineKeyboardButton::callback(
            t!(toggle_mod_key, locale = &locale),
            format!("{}:settings:toggle_module:{}", self.key(), cid),
        );

        let keyboard = InlineKeyboardMarkup::new(vec![
            vec![toggle_mod_btn],
            vec![InlineKeyboardButton::callback(
                t!("modules.speech.btn_models", locale = &locale),
                format!("{}:settings:menu:models:{}", self.key(), cid),
            )],
            vec![
                toggle(
                    t!("modules.speech.type_voice", locale = &locale).to_string(),
                    s.enable_voice,
                    "toggle_voice",
                ),
                toggle(
                    t!("modules.speech.type_video", locale = &locale).to_string(),
                    s.enable_video_note,
                    "toggle_video",
                ),
            ],
            vec![toggle(
                t!("modules.speech.type_audio", locale = &locale).to_string(),
                s.enable_audio,
                "toggle_audio",
            )],
            vec![InlineKeyboardButton::callback(
                t!("common.back", locale = &locale).to_string(),
                format!("settings_back:{}:{}:{}", owner.r#type, owner.id, cid),
            )],
        ]);

        Ok((text, keyboard))
    }

    async fn render_models_menu(
        &self,
        owner: &Owner,
        cid: u64,
    ) -> Result<(String, InlineKeyboardMarkup), MyError> {
        let s: SpeechRecognitionSettings = Settings::get_module_settings(owner, self.key()).await?;

        let config = Config::new().await;
        let locale = get_locale_by_owner(&owner.id, &owner.r#type, &config).await;

        let models = [
            SpeechModel::Gemini25Flash,
            SpeechModel::Gemini25Pro,
            SpeechModel::Gemini3Flash,
        ];

        let trans_lbl = t!("modules.speech.transcription", locale = &locale);
        let sum_lbl = t!("modules.speech.summary", locale = &locale);

        let text = format!(
            "{}\n\n<b>{}:</b> {}\n<b>{}:</b> {}",
            t!("modules.speech.models_header", locale = &locale),
            trans_lbl,
            s.transcription_model.display_name(),
            sum_lbl,
            s.summary_model.display_name()
        );

        let mut rows = Vec::new();

        rows.push(vec![InlineKeyboardButton::callback(
            t!("modules.speech.model_trans_header", locale = &locale),
            "noop",
        )]);

        for model in &models {
            let is_selected = s.transcription_model == *model;
            let label = if is_selected {
                format!("• {} •", model.display_name())
            } else {
                model.display_name().to_string()
            };
            rows.push(vec![InlineKeyboardButton::callback(
                label,
                format!(
                    "{}:settings:set_trans_model:{}:{}",
                    self.key(),
                    model.display_name(),
                    cid
                ),
            )]);
        }

        rows.push(vec![InlineKeyboardButton::callback(
            t!("modules.speech.model_sum_header", locale = &locale),
            "noop",
        )]);

        for model in &models {
            let is_selected = s.summary_model == *model;
            let label = if is_selected {
                format!("• {} •", model.display_name())
            } else {
                model.display_name().to_string()
            };
            rows.push(vec![InlineKeyboardButton::callback(
                label,
                format!(
                    "{}:settings:set_sum_model:{}:{}",
                    self.key(),
                    model.display_name(),
                    cid
                ),
            )]);
        }

        rows.push(vec![InlineKeyboardButton::callback(
            t!("common.back", locale = &locale),
            format!("{}:settings:menu:main:{}", self.key(), cid),
        )]);

        Ok((text, InlineKeyboardMarkup::new(rows)))
    }
}
