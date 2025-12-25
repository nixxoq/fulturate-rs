#[macro_export]
macro_rules! module {
    (
        struct $mod_name:ident;
        settings = $settings:ty;
        key = $key:literal;
        name = $name:literal;
        desc = $desc:literal;
        designed_for = $designed:expr;
        impl {
            $($custom_impl:tt)*
        }
    ) => {
        pub struct $mod_name;

        #[async_trait::async_trait]
        impl $crate::bot::modules::Module for $mod_name {
            fn key(&self) -> &'static str { $key }
            fn name(&self) -> &'static str { $name }
            fn description(&self) -> &'static str { $desc }

            fn designed_for(&self, owner_type: &str) -> bool {
                let target: &str = $designed;
                if target == "all" { true } else { owner_type == target }
            }

            async fn is_enabled(&self, owner: &$crate::bot::modules::Owner) -> bool {
                if !self.designed_for(&owner.r#type) { return false; }
                let settings: $settings =
                    $crate::core::db::schemas::settings::Settings::get_module_settings(owner, self.key())
                    .await.unwrap_or_default();
                settings.enabled
            }

            fn factory_settings(&self) -> Result<serde_json::Value, $crate::errors::MyError> {
                let s = <$settings>::default();
                Ok(serde_json::to_value(s)?)
            }

            $($custom_impl)*
        }
    };

    (
        struct $struct_name:ident;
        key = $key:literal;
        name = $name:literal;
        desc = $desc:literal;
        designed_for = $designed:expr;
    ) => {
        $crate::module! {
            struct $struct_name;
            settings = $crate::bot::modules::SimpleModuleSettings;
            key = $key;
            name = $name;
            desc = $desc;
            designed_for = $designed;

            impl {
                async fn get_settings_ui(
                    &self,
                    owner: &$crate::bot::modules::Owner,
                    commander_id: u64,
                ) -> Result<(String, teloxide::types::InlineKeyboardMarkup), $crate::errors::MyError> {
                    use $crate::{
                        core::{db::schemas::settings::Settings, config::Config},
                        bot::modules::SimpleModuleSettings,
                        util::i18n::get_locale_by_owner,
                        t,
                    };
                    use teloxide::types::{InlineKeyboardMarkup, InlineKeyboardButton};

                    let config = Config::new().await;
                    let locale = get_locale_by_owner(&owner.id, &owner.r#type, &config).await;

                    let settings: SimpleModuleSettings = Settings::get_module_settings(owner, self.key()).await?;

                    let name_key = format!("modules.{}.name", self.key());
                    let desc_key = format!("modules.{}.desc", self.key());

                    let tr_name = t!(&name_key, locale = &locale);
                    let module_name = if tr_name == name_key { self.name() } else { &tr_name };

                    let tr_desc = t!(&desc_key, locale = &locale);
                    let module_desc = if tr_desc == desc_key { self.description() } else { &tr_desc };

                    let status_key = if settings.enabled { "modules.status_on" } else { "modules.status_off" };
                    let status_text = t!(status_key, locale = &locale);

                    let text = t!("modules.status_header",
                        locale = &locale,
                        name = module_name,
                        desc = module_desc,
                        status = status_text
                    );

                    let toggle_key = if settings.enabled { "settings.toggle_off" } else { "settings.toggle_on" };
                    let toggle_btn = InlineKeyboardButton::callback(
                        t!(toggle_key, locale = &locale),
                        format!("{}:settings:toggle_module:{}", self.key(), commander_id),
                    );

                    let back_btn = InlineKeyboardButton::callback(
                        t!("common.back", locale = &locale),
                        format!("settings_back:{}:{}:{}", owner.r#type, owner.id, commander_id),
                    );

                    let keyboard = InlineKeyboardMarkup::new(vec![
                        vec![toggle_btn],
                        vec![back_btn],
                    ]);

                    Ok((text.to_string(), keyboard))
                }

                async fn handle_callback(
                    &self,
                    bot: teloxide::Bot,
                    q: &teloxide::types::CallbackQuery,
                    owner: &$crate::bot::modules::Owner,
                    data: &str,
                    commander_id: u64,
                ) -> Result<(), $crate::errors::MyError> {
                    use $crate::bot::modules::{SimpleModuleSettings, save_and_refresh};
                    use $crate::core::db::schemas::settings::Settings;

                    let Some(message) = &q.message else { return Ok(()); };
                    let Some(message) = message.regular_message() else { return Ok(()); };

                    let parts: Vec<_> = data.split(':').collect();

                    if parts.len() >= 1 && parts[0] == "toggle_module" {
                        let mut settings: SimpleModuleSettings = Settings::get_module_settings(owner, self.key()).await?;
                        settings.enabled = !settings.enabled;
                        save_and_refresh(&bot, message, owner, self, settings, commander_id).await?;
                        return Ok(());
                    }

                    bot.answer_callback_query(q.id.clone()).await?;
                    Ok(())
                }
            }
        }
    };
}
