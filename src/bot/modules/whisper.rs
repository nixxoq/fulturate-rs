use crate::bot::modules::SimpleModuleSettings;
use teloxide::prelude::*;

pub type WhisperSettings = SimpleModuleSettings;

module! {
    struct WhisperModule;
    key = "whisper";
    name = "Whisper System";
    desc = "Модуль «шептать», позволяющий работать с текстовыми сообщениями в более приватном режиме. Протестировать можно через inlin'ы: \"@fulturatebot *сообщение шепота* @username1 *id*\"";
    designed_for = "user";
}
