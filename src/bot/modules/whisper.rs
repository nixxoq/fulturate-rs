use crate::bot::modules::ModuleSettings;
use serde::{Deserialize, Serialize};
use teloxide::prelude::*;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct WhisperSettings {
    pub enabled: bool,
}

impl ModuleSettings for WhisperSettings {}

module! {
    struct WhisperModule;
    settings = WhisperSettings;
    key = "whisper";
    name = "Whisper System";
    desc = "Модуль «шептать», позволяющий работать с текстовыми сообщениями в более приватном режиме. Протестировать можно через inlin'ы: \"@fulturatebot *сообщение шепота* @username1 *id*\"";
    designed_for = "user";
}
