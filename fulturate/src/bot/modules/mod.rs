#[macro_use]
pub mod macros;

pub mod cobalt;
pub mod currency;
pub mod math;
pub mod registry;
pub mod translate;
pub mod whisper;

use crate::{core::db::schemas::settings::Settings, errors::MyError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fmt::Debug;
use teloxide::{
    payloads::EditMessageTextSetters,
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
};

#[derive(Clone, Debug)]
pub struct Owner {
    pub id: String,
    pub r#type: String, // "user", "group"
}

#[async_trait]
pub trait ModuleSettings:
    Sized + Default + Serialize + DeserializeOwned + Debug + Send + Sync + 'static
{
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SimpleModuleSettings {
    pub enabled: bool,
}

impl Default for SimpleModuleSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl ModuleSettings for SimpleModuleSettings {}

#[async_trait]
pub trait Module: Send + Sync {
    fn key(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;

    async fn get_settings_ui(
        &self,
        owner: &Owner,
        commander_id: u64,
    ) -> Result<(String, InlineKeyboardMarkup), MyError>;

    async fn handle_callback(
        &self,
        bot: Bot,
        q: &CallbackQuery,
        owner: &Owner,
        data: &str,
        commander_id: u64,
    ) -> Result<(), MyError>;

    fn designed_for(&self, owner_type: &str) -> bool;

    async fn is_enabled(&self, owner: &Owner) -> bool;

    fn factory_settings(&self) -> Result<serde_json::Value, MyError>;
}

// TODO: i18n stuff
pub fn standard_settings_header(name: &str, desc: &str, enabled: bool) -> String {
    format!(
        "⚙️ <b>Настройки модуля</b>: {}\n<blockquote>{}</blockquote>\nСтатус: {}",
        name,
        desc,
        if enabled {
            "✅ Включен"
        } else {
            "❌ Выключен"
        }
    )
}

pub fn standard_toggle_button(key: &str, enabled: bool, commander_id: u64) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(
        if enabled {
            "Выключить модуль"
        } else {
            "Включить модуль"
        },
        format!("{}:settings:toggle_module:{}", key, commander_id),
    )
}

pub fn standard_back_button(owner: &Owner, commander_id: u64) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(
        "⬅️ Назад",
        format!(
            "settings_back:{}:{}:{}",
            owner.r#type, owner.id, commander_id
        ),
    )
}

pub fn create_radio_row<T, F>(
    current_val: &T,
    options: &[T],
    base_callback: &str,
    display_fn: F,
) -> Vec<InlineKeyboardButton>
where
    T: PartialEq,
    F: Fn(&T) -> String,
{
    options
        .iter()
        .map(|opt| {
            let label = if current_val == opt {
                format!("• {} •", display_fn(opt))
            } else {
                display_fn(opt)
            };
            let cb_data = format!("{}:{}", base_callback, display_fn(opt));
            InlineKeyboardButton::callback(label, cb_data)
        })
        .collect()
}

pub async fn save_and_refresh<M, S>(
    bot: &Bot,
    message: &Message,
    owner: &Owner,
    module: &M,
    new_settings: S,
    commander_id: u64,
) -> Result<(), MyError>
where
    M: Module + ?Sized,
    S: ModuleSettings,
{
    Settings::update_module_settings(owner, module.key(), new_settings).await?;
    let (text, keyboard) = module.get_settings_ui(owner, commander_id).await?;

    bot.edit_message_text(message.chat.id, message.id, text)
        .reply_markup(keyboard)
        .parse_mode(teloxide::types::ParseMode::Html)
        .await?;
    Ok(())
}
