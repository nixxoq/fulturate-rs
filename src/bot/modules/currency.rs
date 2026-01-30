use crate::{
    bot::modules::{Module, ModuleSettings, Owner},
    core::{
        config::Config,
        db::schemas::{group::Group, settings::Settings, user::User as DbUser},
        services::{
            currencier::handle_currency_update,
            currency::converter::{
                CURRENCY_CONFIG_PATH, get_all_currency_codes, get_default_currencies,
            },
        },
    },
    errors::MyError,
    t,
    util::{
        i18n::get_locale_by_owner,
        paginator::{ItemsBuild, Paginator},
    },
};
use serde::{Deserialize, Serialize};
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup},
};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CurrencySettings {
    pub enabled: bool,
    pub selected_codes: Vec<String>,
}

impl Default for CurrencySettings {
    fn default() -> Self {
        let default_currencies = get_default_currencies()
            .map(|currencies| {
                currencies
                    .into_iter()
                    .map(|c| c.code)
                    .collect::<Vec<String>>()
            })
            .unwrap_or_else(|_| vec!["usd".to_string(), "eur".to_string()]);

        Self {
            enabled: true,
            selected_codes: default_currencies,
        }
    }
}

impl ModuleSettings for CurrencySettings {}

module! {
    struct CurrencyModule;
    settings = CurrencySettings;
    key = "currency";
    name = "Конвертер валют";
    desc = "Конвертация валют и криптовалют с актуальными курсами";
    designed_for = "all";

    impl {
        async fn get_settings_ui(
            &self,
            owner: &Owner,
            commander_id: u64,
        ) -> Result<(String, InlineKeyboardMarkup), MyError> {
            self.get_paged_settings_ui(owner, 0, commander_id).await
        }

        async fn handle_callback(
            &self,
            bot: Bot,
            q: &CallbackQuery,
            owner: &Owner,
            data: &str,
            commander_id: u64,
        ) -> Result<(), MyError> {
            let Some(message) = &q.message else { return Ok(()); };
            let Some(message) = message.regular_message() else { return Ok(()); };

            let parts: Vec<_> = data.split(':').collect();

            if parts.is_empty() { return Ok(()); }

            let mut s: CurrencySettings = Settings::get_module_settings(owner, self.key()).await?;
            let mut changed = false;
            let mut page = 0;

            println!("{}", parts[0]);

            match parts[0] {
                "toggle_module" => {
                    s.enabled = !s.enabled;
                    if s.enabled && s.selected_codes.is_empty() {
                         s.selected_codes = vec![
                            "UAH".to_string(), "RUB".to_string(), "USD".to_string(),
                            "BYN".to_string(), "EUR".to_string(), "TON".to_string(),
                        ];
                    }
                    changed = true;
                }
                "page" if parts.len() >= 2 => {
                    page = parts[1].parse().unwrap_or(0);
                    changed = true;
                }
                "toggle" if parts.len() >= 2 => {
                    let code = parts[1].to_string();
                    if let Some(pos) = s.selected_codes.iter().position(|c| *c == code) {
                        s.selected_codes.remove(pos);
                    } else {
                        s.selected_codes.push(code);
                    }
                    changed = true;
                }
                _ => {}
            }

            if changed {
                Settings::update_module_settings(owner, self.key(), s).await?;
                let (text, keyboard) = self.get_paged_settings_ui(owner, page, commander_id).await?;

                bot.edit_message_text(message.chat.id, message.id, text)
                    .reply_markup(keyboard)
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
            } else {
                bot.answer_callback_query(q.id.clone()).await?;
            }

            Ok(())
        }
    }
}

impl CurrencyModule {
    async fn get_paged_settings_ui(
        &self,
        owner: &Owner,
        page: usize,
        commander_id: u64,
    ) -> Result<(String, InlineKeyboardMarkup), MyError> {
        let config = Config::new().await;
        let locale = get_locale_by_owner(&owner.id, &owner.r#type, &config).await;

        let settings: CurrencySettings = Settings::get_module_settings(owner, self.key()).await?;

        let status_key = if settings.enabled {
            "modules.status_on"
        } else {
            "modules.status_off"
        };
        let status_text = t!(status_key, locale = &locale);

        let module_name = t!("modules.currency.name", locale = &locale);
        let module_desc = t!("modules.currency.desc", locale = &locale);

        let header_info = t!(
            "modules.status_header",
            locale = &locale,
            name = module_name,
            desc = module_desc,
            status = status_text
        );

        let select_text = t!("modules.currency.select_header", locale = &locale);

        let text = format!("{}\n\n{}", header_info, select_text);

        let toggle_key = if settings.enabled {
            "settings.toggle_off"
        } else {
            "settings.toggle_on"
        };
        let toggle_btn = InlineKeyboardButton::callback(
            t!(toggle_key, locale = &locale),
            format!("{}:settings:toggle_module:{}", self.key(), commander_id),
        );

        let back_btn = InlineKeyboardButton::callback(
            t!("common.back", locale = &locale),
            format!(
                "settings_back:{}:{}:{}",
                owner.r#type, owner.id, commander_id
            ),
        );

        let all_currencies = get_all_currency_codes(CURRENCY_CONFIG_PATH.parse().unwrap())?;

        let mut keyboard = Paginator::from(self.key(), &all_currencies)
            .per_page(12)
            .columns(3)
            .current_page(page)
            .add_bottom_row(vec![back_btn])
            .set_callback_prefix(format!("{}:settings", self.key()))
            .set_callback_formatter(move |p| {
                format!("{}:settings:page:{}:{}", self.key(), p, commander_id)
            })
            .build(|currency| {
                let is_selected = settings.selected_codes.contains(&currency.code);
                let icon = if is_selected { "✅" } else { "❌" };
                let label = format!("{} {}", icon, currency.code);
                let cb_data = format!(
                    "{}:settings:toggle:{}:{}",
                    self.key(),
                    currency.code,
                    commander_id
                );
                InlineKeyboardButton::callback(label, cb_data)
            });

        keyboard.inline_keyboard.insert(0, vec![toggle_btn]);

        Ok((text, keyboard))
    }
}

pub async fn currency_codes_handler(
    bot: Bot,
    msg: Message,
    code: String,
    config: &Config,
) -> Result<(), MyError> {
    if msg.chat.is_private() {
        handle_currency_update::<DbUser>(bot, msg, code, config).await
    } else {
        handle_currency_update::<Group>(bot, msg, code, config).await
    }
}
