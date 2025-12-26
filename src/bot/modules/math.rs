use crate::bot::modules::SimpleModuleSettings;
use teloxide::prelude::*;

pub type MathSettings = SimpleModuleSettings;

module! {
    struct MathModule;
    key = "math";
    name = "Math";
    desc = "Модуль математических выражений..."; // todo: finish this
    designed_for = "none";
}
