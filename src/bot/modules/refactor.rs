use crate::bot::modules::SimpleModuleSettings;
use teloxide::prelude::*;

pub type RefactorSettings = SimpleModuleSettings;

module! {
    struct RefactorModule;
    key = "refactor";
    name = "Text Refactor";
    desc = "Модуль для редактирования, улучшения и анализа текста с помощью ИИ.";
    designed_for = "all";
}
