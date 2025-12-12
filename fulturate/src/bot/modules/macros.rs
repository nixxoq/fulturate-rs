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
            // FIX: Changed $mod_name to $name
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
                    use $crate::bot::modules::{standard_settings_header, standard_toggle_button, standard_back_button};
                    use $crate::core::db::schemas::settings::Settings;
                    use teloxide::types::InlineKeyboardMarkup;
                    use $crate::bot::modules::SimpleModuleSettings; // Explicit import to avoid ambiguity

                    let settings: SimpleModuleSettings = Settings::get_module_settings(owner, self.key()).await?;

                    let text = standard_settings_header(self.name(), self.description(), settings.enabled);
                    let keyboard = InlineKeyboardMarkup::new(vec![
                        vec![standard_toggle_button(self.key(), settings.enabled, commander_id)],
                        vec![standard_back_button(owner, commander_id)],
                    ]);

                    Ok((text, keyboard))
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
