use crate::{
    bot::modules::{ModuleSettings, Owner, save_and_refresh},
    core::{
        config::Config,
        db::schemas::settings::Settings,
        services::cobalt::{AudioQuality, VideoQuality},
    },
    errors::MyError,
    t,
    util::i18n::get_locale_by_owner,
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
            enabled: true,
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
                        let config = Config::new().await;
            let locale = get_locale_by_owner(&owner.id, &owner.r#type, &config).await;

            // let text = standard_settings_header(self.name(), self.description(), s.enabled);
            let module_name = t!("modules.cobalt.name", locale = &locale);
            let module_desc = t!("modules.cobalt.desc", locale = &locale);
            let status_key = if s.enabled { "modules.status_on" } else { "modules.status_off" };
            let status_text = t!(status_key, locale = &locale);

            let text = t!("modules.status_header",
                locale = &locale,
                name = module_name,
                desc = module_desc,
                status = status_text
            );

            let create_radio_row = |opts: &[&str], current: &str, prefix: &str| -> Vec<InlineKeyboardButton> {
                opts.iter().map(|opt| {
                    let label = if *opt == current {
                        format!("• {} •", opt)
                    } else {
                        opt.to_string()
                    };
                    let cb_data = format!("{}:{}:{}", prefix, opt, cid);
                    InlineKeyboardButton::callback(label, cb_data)
                }).collect()
            };

            let vid_opts = [VideoQuality::Q720, VideoQuality::Q1080, VideoQuality::Q1440, VideoQuality::Max];
            let aud_opts = [AudioQuality::K128, AudioQuality::K256, AudioQuality::K320];

            let vid_opts_str: Vec<String> = vid_opts.iter().map(|v| v.as_str().to_string()).collect();
            let vid_opts_ref: Vec<&str> = vid_opts_str.iter().map(|v| v.as_str()).collect();
            let vid_row = create_radio_row(
                &vid_opts_ref,
                s.video_quality.as_str(),
                &format!("{}:settings:set:video", self.key())
            );

            let aud_opts_str: Vec<String> = aud_opts.iter().map(|a| a.as_str().to_string()).collect();
            let aud_opts_ref: Vec<&str> = aud_opts_str.iter().map(|a| a.as_str()).collect();
            let aud_row = create_radio_row(
                &aud_opts_ref,
                s.audio_quality.as_str(),
                &format!("{}:settings:set:audio", self.key())
            );

            let attr_key = if s.attribution { "modules.cobalt.attr_on" } else { "modules.cobalt.attr_off" };
            let attr_btn = InlineKeyboardButton::callback(
                t!(attr_key, locale = &locale),
                format!("{}:settings:set:attr:{}:{}", self.key(), !s.attribution, cid)
            );

            let toggle_key = if s.enabled { "settings.toggle_off" } else { "settings.toggle_on" };
            let toggle_btn = InlineKeyboardButton::callback(
                t!(toggle_key, locale = &locale),
                format!("{}:settings:toggle_module:{}", self.key(), cid)
            );

            let keyboard = InlineKeyboardMarkup::new(vec![
                vec![toggle_btn],
                vec![InlineKeyboardButton::callback(t!("modules.cobalt.btn_quality_video", locale = &locale), "noop")],
                vid_row,
                vec![InlineKeyboardButton::callback(t!("modules.cobalt.btn_quality_audio", locale = &locale), "noop")],
                aud_row,
                vec![attr_btn],
                vec![InlineKeyboardButton::callback(
                    t!("common.back", locale = &locale),
                    format!("settings_back:{}:{}:{}", owner.r#type, owner.id, cid),
                )],
            ]);

            Ok((text.to_string(), keyboard))
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
