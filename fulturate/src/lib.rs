use rust_i18n::i18n;

i18n!("locales", fallback = "en");

pub mod bot;
pub mod core;
pub mod errors;
pub mod util;
