use crate::{
    bot::modules::{
        ModuleSettings, Owner, create_radio_row, save_and_refresh, standard_back_button,
        standard_settings_header, standard_toggle_button,
    },
    core::{
        db::schemas::settings::Settings,
        services::cobalt::{AudioQuality, VideoQuality},
    },
    errors::MyError,
    module,
};
use serde::{Deserialize, Serialize};
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CobaltSettings {
    pub enabled: bool,
    pub video_quality: VideoQuality,
    pub attribution: bool,
    #[serde(default)]
    pub audio_quality: AudioQuality,
}

impl Default for CobaltSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            video_quality: VideoQuality::Q720,
            audio_quality: AudioQuality::K256,
            attribution: false,
        }
    }
}
impl ModuleSettings for CobaltSettings {}

module! {
    struct CobaltModule;
    settings = CobaltSettings;
    key = "cobalt";
    name = "Cobalt Downloader";
    desc = "Скачивание медиа с популярных платформ.";
    designed_for = "user";

    impl {
        async fn get_settings_ui(
            &self,
            owner: &Owner,
            cid: u64,
        ) -> Result<(String, InlineKeyboardMarkup), MyError> {
            let s: CobaltSettings = Settings::get_module_settings(owner, self.key()).await?;

            let text = standard_settings_header(self.name(), self.description(), s.enabled);

            let vid_opts = [VideoQuality::Q720, VideoQuality::Q1080, VideoQuality::Q1440, VideoQuality::Max];
            let aud_opts = [AudioQuality::K128, AudioQuality::K256, AudioQuality::K320];

            let vid_row = create_radio_row(
                &s.video_quality,
                &vid_opts,
                &format!("{}:settings:set:video", self.key()),
                |v| v.as_str().to_string()
            );

            let aud_row = create_radio_row(
                &s.audio_quality,
                &aud_opts,
                &format!("{}:settings:set:audio", self.key()),
                |a| a.as_str().to_string()
            );

            let attr_txt = if s.attribution { "Атрибуция: Вкл ✅" } else { "Атрибуция: Выкл ❌" };
            let attr_btn = InlineKeyboardButton::callback(
                attr_txt,
                format!("{}:settings:set:attr:{}:{}", self.key(), !s.attribution, cid)
            );

            let keyboard = InlineKeyboardMarkup::new(vec![
                vec![standard_toggle_button(self.key(), s.enabled, cid)],
                vec![InlineKeyboardButton::callback("Качество видео", "noop")],
                vid_row,
                vec![InlineKeyboardButton::callback("Качество аудио", "noop")],
                aud_row,
                vec![attr_btn],
                vec![standard_back_button(owner, cid)],
            ]);

            Ok((text, keyboard))
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

            let mut s: CobaltSettings = Settings::get_module_settings(owner, self.key()).await?;
            let mut changed = false;

            match parts[0] {
                "toggle_module" => { s.enabled = !s.enabled; changed = true; }
                "set" if parts.len() >= 3 => {
                    match parts[1] {
                        "video" => s.video_quality = VideoQuality::parse_quality(parts[2]),
                        "audio" => s.audio_quality = AudioQuality::parse_quality(parts[2]),
                        "attr" => s.attribution = parts[2].parse().unwrap_or(false),
                        _ => return Ok(())
                    }
                    changed = true;
                }
                _ => {}
            }

            if changed {
                save_and_refresh(&bot, msg, owner, self, s, cid).await?;
            } else {
                bot.answer_callback_query(q.clone().id).await?;
            }

            Ok(())
        }
    }
}
