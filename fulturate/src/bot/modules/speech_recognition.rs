use crate::{
    bot::modules::{
        Module, ModuleSettings, Owner, create_radio_row, standard_back_button,
        standard_settings_header, standard_toggle_button,
    },
    core::db::schemas::settings::Settings,
    errors::MyError,
    module,
};
use serde::{Deserialize, Serialize};
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum SpeechModel {
    Gemini25Flash,
    Gemini30Flash,
    Gemini25Pro,
}

impl SpeechModel {
    pub fn display_name(&self) -> &str {
        match self {
            Self::Gemini25Flash => "Gemini 2.5 Flash",
            Self::Gemini30Flash => "Gemini 3 Flash",
            Self::Gemini25Pro => "Gemini 2.5 Pro",
        }
    }

    pub fn api_key(&self) -> &str {
        match self {
            Self::Gemini25Flash => "gemini-2.5-flash",
            Self::Gemini25Pro => "gemini-2.5-pro",
            Self::Gemini30Flash => "gemini-3-flash",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "Gemini 2.5 Pro" => Self::Gemini25Pro,
            "Gemini 3 Flash" => Self::Gemini30Flash,
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
    desc = "Модуль для обработки, расшифровки и пересказа голосовы  х сообщений.";
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

        let text = format!(
            "{}\n\n<b>🎙 Расшифровка:</b> {}\n<b>📝 Пересказ:</b> {}\n\nВыберите типы сообщений:",
            standard_settings_header(self.name(), self.description(), s.enabled),
            s.transcription_model.display_name(),
            s.summary_model.display_name()
        );

        let toggle = |lbl: &str, val: bool, cb: &str| {
            InlineKeyboardButton::callback(
                format!("{} {}", if val { "✅" } else { "❌" }, lbl),
                format!("{}:settings:{}:{}", self.key(), cb, cid),
            )
        };

        let keyboard = InlineKeyboardMarkup::new(vec![
            vec![standard_toggle_button(self.key(), s.enabled, cid)],
            vec![InlineKeyboardButton::callback(
                "🤖 Настроить модели",
                format!("{}:settings:menu:models:{}", self.key(), cid),
            )],
            vec![
                toggle("Голосовые", s.enable_voice, "toggle_voice"),
                toggle("Кружочки", s.enable_video_note, "toggle_video"),
            ],
            vec![toggle("Аудио файлы", s.enable_audio, "toggle_audio")],
            vec![standard_back_button(owner, cid)],
        ]);

        Ok((text, keyboard))
    }

    async fn render_models_menu(
        &self,
        owner: &Owner,
        cid: u64,
    ) -> Result<(String, InlineKeyboardMarkup), MyError> {
        let s: SpeechRecognitionSettings = Settings::get_module_settings(owner, self.key()).await?;
        let models = [SpeechModel::Gemini25Flash, SpeechModel::Gemini25Pro, SpeechModel::Gemini30Flash];

        let text = format!(
            "🤖 <b>Настройка моделей</b>\n\n<b>Расшифровка:</b> {}\n<b>Пересказ/Итоги:</b> {}",
            s.transcription_model.display_name(),
            s.summary_model.display_name()
        );

        let mut rows = Vec::new();

        rows.push(vec![InlineKeyboardButton::callback(
            "🎙 Модель для расшифровки",
            "noop",
        )]);
        let trans_rows = create_radio_row(
            &s.transcription_model,
            &models,
            &format!("{}:settings:set_trans_model", self.key()),
            cid,
            |m| m.display_name().to_string(),
        );
        for btn in trans_rows {
            rows.push(vec![btn]);
        }

        rows.push(vec![InlineKeyboardButton::callback(
            "📝 Модель для пересказа",
            "noop",
        )]);
        let sum_rows = create_radio_row(
            &s.summary_model,
            &models,
            &format!("{}:settings:set_sum_model", self.key()),
            cid,
            |m| m.display_name().to_string(),
        );
        for btn in sum_rows {
            rows.push(vec![btn]);
        }

        rows.push(vec![InlineKeyboardButton::callback(
            "⬅️ Назад",
            format!("{}:settings:menu:main:{}", self.key(), cid),
        )]);

        Ok((text, InlineKeyboardMarkup::new(rows)))
    }
}
