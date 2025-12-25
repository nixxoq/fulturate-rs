use crate::bot::modules::SimpleModuleSettings;
use teloxide::prelude::*;

pub type TranslateSettings = SimpleModuleSettings;

module! {
    struct TranslateModule;
    key = "translate";
    name = "Translate";
    desc = "Модуль перевода, позволяющий быстро получить перевод введенного текста. Протестировать можно через inlin'ы: \"@fulturatebot *слова или фраза для перевода*\"";
    designed_for = "user";
}
