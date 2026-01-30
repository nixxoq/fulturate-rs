use crate::{
    bot::modules::{Module, ModuleSettings, Owner},
    core::{config::Config, db::schemas::settings::Settings, services::translation::Engine},
    errors::MyError,
    t,
    util::i18n::get_locale_by_owner,
};
use serde::{Deserialize, Serialize};
use teloxide::{
    Bot,
    payloads::EditMessageTextSetters,
    prelude::{CallbackQuery, Requester},
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranslateSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub default_engine: Engine,
}

impl Default for TranslateSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            default_engine: Engine::Google,
        }
    }
}

impl ModuleSettings for TranslateSettings {}

module! {
    struct TranslateModule;
    settings = TranslateSettings;
    key = "translate";
    name = "Translate";
    desc = "Модуль перевода, позволяющий быстро получить перевод введенного текста. Протестировать можно через inlin'ы: \"@fulturatebot *слова или фраза для перевода*\"";
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

            let mut s: TranslateSettings = Settings::get_module_settings(owner, self.key()).await?;
            let mut save = false;
            let mut view = "main";

            match parts[0] {
                "toggle_module" => {
                    s.enabled = !s.enabled;
                    save = true;
                }

                "menu" if parts.len() >= 2 => {
                    view = parts[1];

                    let (text, keyboard) = match view {
                        "engines" => self.render_engine_menu(owner, cid).await?,
                        _ => self.render_main_menu(owner, cid).await?,
                    };
                    bot.edit_message_text(msg.chat.id, msg.id, text)
                        .reply_markup(keyboard)
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                    return Ok(());
                }

                "set_engine" if parts.len() >= 2 => {
                    let engine_str = parts[1];
                    if let Some(eng) = Engine::all().iter().find(|e| e.as_str() == engine_str) {
                        s.default_engine = *eng;
                        save = true;
                        view = "engines";
                    }
                }
                _ => {}
            }

            if save {
                Settings::update_module_settings(owner, self.key(), s).await?;

                let (text, keyboard) = match view {
                    "engines" => self.render_engine_menu(owner, cid).await?,
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

impl TranslateModule {
    async fn render_main_menu(
        &self,
        owner: &Owner,
        cid: u64,
    ) -> Result<(String, InlineKeyboardMarkup), MyError> {
        let s: TranslateSettings = Settings::get_module_settings(owner, self.key()).await?;
        let config = Config::new().await;
        let locale = get_locale_by_owner(&owner.id, &owner.r#type, &config).await;

        let module_name = t!(
            format!("modules.{}.name", self.key()).as_str(),
            locale = &locale
        );
        let module_desc = t!(
            format!("modules.{}.desc", self.key()).as_str(),
            locale = &locale
        );

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

        let current_engine = s.default_engine.display_name();
        let text = format!("{}\n\n<b>Current Engine:</b> {}", header, current_engine);

        let toggle_key = if s.enabled {
            "settings.toggle_off"
        } else {
            "settings.toggle_on"
        };
        let toggle_btn = InlineKeyboardButton::callback(
            t!(toggle_key, locale = &locale),
            format!("{}:settings:toggle_module:{}", self.key(), cid),
        );

        let engine_btn = InlineKeyboardButton::callback(
            format!("Engine: {}", current_engine),
            format!("{}:settings:menu:engines:{}", self.key(), cid),
        );

        let back_btn = InlineKeyboardButton::callback(
            t!("common.back", locale = &locale),
            format!("settings_back:{}:{}:{}", owner.r#type, owner.id, cid),
        );

        let keyboard =
            InlineKeyboardMarkup::new(vec![vec![toggle_btn], vec![engine_btn], vec![back_btn]]);

        Ok((text, keyboard))
    }

    async fn render_engine_menu(
        &self,
        owner: &Owner,
        cid: u64,
    ) -> Result<(String, InlineKeyboardMarkup), MyError> {
        let s: TranslateSettings = Settings::get_module_settings(owner, self.key()).await?;
        let config = Config::new().await;
        let locale = get_locale_by_owner(&owner.id, &owner.r#type, &config).await;

        let text = "<b>Select Translation Engine:</b>".to_string();

        let engines = Engine::all();
        let mut rows = Vec::new();

        for chunk in engines.chunks(2) {
            let row = chunk
                .iter()
                .map(|eng| {
                    let label = if s.default_engine == *eng {
                        format!("✅ {}", eng.display_name())
                    } else {
                        eng.display_name().to_string()
                    };

                    InlineKeyboardButton::callback(
                        label,
                        format!(
                            "{}:settings:set_engine:{}:{}",
                            self.key(),
                            eng.as_str(),
                            cid
                        ),
                    )
                })
                .collect();
            rows.push(row);
        }

        rows.push(vec![InlineKeyboardButton::callback(
            t!("common.back", locale = &locale),
            format!("{}:settings:menu:main:{}", self.key(), cid),
        )]);

        Ok((text, InlineKeyboardMarkup::new(rows)))
    }
}
