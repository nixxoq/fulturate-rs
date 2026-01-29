use crate::bot::commands::help::handle_help_pagination_callback;
use crate::core::db::schemas::settings::Settings;
use crate::util::i18n::get_locale_by_owner;
use crate::{
    bot::{
        callbacks::{
            cobalt_pagination::handle_cobalt_pagination,
            delete::{
                handle_delete_confirmation, handle_delete_data, handle_delete_data_confirmation,
                handle_delete_request,
            },
            translate::handle_translate_callback,
            whisper::handle_whisper_callback,
        },
        commands::settings::{
            get_lang_settings_menu, get_main_settings_menu, get_modules_settings_menu,
        },
        modules::{Owner, registry::MOD_MANAGER},
    },
    core::{
        config::Config,
        services::speech_recognition::{
            back_handler, pagination_handler, retry_speech_handler, summarization_handler,
        },
    },
    errors::MyError,
    t,
    util::i18n::{get_user_locale, set_locale},
};
use log::error;
use std::sync::Arc;
use teloxide::{
    Bot,
    payloads::{AnswerCallbackQuerySetters, EditMessageTextSetters},
    prelude::{CallbackQuery, Requester},
    types::MaybeInaccessibleMessage,
};

pub mod admin;
pub mod cobalt_pagination;
pub mod delete;
pub mod refactor;
pub mod translate;
pub mod whisper;

enum CallbackAction<'a> {
    SettingsMain {
        owner_type: &'a str,
        owner_id: &'a str,
        commander_id: u64,
    },
    ToggleAdminOnly {
        owner_type: &'a str,
        owner_id: &'a str,
        commander_id: u64,
    },
    SettingsModules {
        owner_type: &'a str,
        owner_id: &'a str,
        commander_id: u64,
    },
    SettingsLang {
        owner_type: &'a str,
        owner_id: &'a str,
        commander_id: u64,
    },
    LangSet {
        lang_code: &'a str,
        owner_type: &'a str,
        owner_id: &'a str,
        commander_id: u64,
    },
    ModuleSettings {
        module_key: &'a str,
        rest: &'a str,
        commander_id: u64,
    },
    ModuleSelect {
        owner_type: &'a str,
        owner_id: &'a str,
        module_key: &'a str,
        commander_id: u64,
    },
    SettingsBack {
        owner_type: &'a str,
        owner_id: &'a str,
        commander_id: u64,
    },
    DeleteData {
        commander_id: u64,
    },
    CobaltPagination,
    DeleteDataConfirmation,
    DeleteMessage,
    DeleteMessageConfirmation,
    Summarize,
    Retell,
    RetrySpeech {
        message_id: i32,
        action_type: &'a str,
        attempt: u32,
    },
    TranscriptionPagination,
    BackToFull,
    Whisper,
    Translate,
    HelpPagination {
        page: usize,
        user_id: u64,
    },
    Admin,
    NoOp,
    Refactor {
        mode: &'a str,
        src_msg_id: i32,
    },
}

fn parse_callback_data(data: &'_ str) -> Option<CallbackAction<'_>> {
    if data == "noop" {
        return Some(CallbackAction::NoOp);
    }

    if let Some(rest) = data.strip_prefix("settings_main:") {
        let parts: Vec<_> = rest.split(':').collect();
        if parts.len() == 3
            && let Ok(commander_id) = parts[2].parse()
        {
            return Some(CallbackAction::SettingsMain {
                owner_type: parts[0],
                owner_id: parts[1],
                commander_id,
            });
        }
    }

    if let Some(rest) = data.strip_prefix("settings_toggle_admin:") {
        let parts: Vec<_> = rest.split(':').collect();
        if parts.len() == 3
            && let Ok(commander_id) = parts[2].parse()
        {
            return Some(CallbackAction::ToggleAdminOnly {
                owner_type: parts[0],
                owner_id: parts[1],
                commander_id,
            });
        }
    }

    if let Some(rest) = data.strip_prefix("settings_modules:") {
        let parts: Vec<_> = rest.split(':').collect();
        if parts.len() == 3
            && let Ok(commander_id) = parts[2].parse()
        {
            return Some(CallbackAction::SettingsModules {
                owner_type: parts[0],
                owner_id: parts[1],
                commander_id,
            });
        }
    }

    if let Some(rest) = data.strip_prefix("settings_lang:") {
        let parts: Vec<_> = rest.split(':').collect();
        if parts.len() == 3
            && let Ok(commander_id) = parts[2].parse()
        {
            return Some(CallbackAction::SettingsLang {
                owner_type: parts[0],
                owner_id: parts[1],
                commander_id,
            });
        }
    }

    if let Some(rest) = data.strip_prefix("lang_set:") {
        let parts: Vec<_> = rest.split(':').collect();
        if parts.len() == 4
            && let Ok(commander_id) = parts[3].parse()
        {
            return Some(CallbackAction::LangSet {
                lang_code: parts[0],
                owner_type: parts[1],
                owner_id: parts[2],
                commander_id,
            });
        }
    }

    if let Some(rest) = data.strip_prefix("module_select:") {
        let parts: Vec<_> = rest.split(':').collect();
        if parts.len() == 4
            && let Ok(commander_id) = parts[3].parse()
        {
            return Some(CallbackAction::ModuleSelect {
                owner_type: parts[0],
                owner_id: parts[1],
                module_key: parts[2],
                commander_id,
            });
        }
    }

    if let Some(rest) = data.strip_prefix("settings_back:") {
        let parts: Vec<_> = rest.split(':').collect();
        if parts.len() == 3
            && let Ok(commander_id) = parts[2].parse()
        {
            return Some(CallbackAction::SettingsBack {
                owner_type: parts[0],
                owner_id: parts[1],
                commander_id,
            });
        }
    }

    if let Some(module_key) = MOD_MANAGER.get_all_modules().iter().find_map(|m| {
        data.starts_with(&format!("{}:settings:", m.key()))
            .then_some(m.key())
    }) {
        let full_rest = data
            .strip_prefix(&format!("{}:settings:", module_key))
            .unwrap_or_default();

        if let Some((rest, id_str)) = full_rest.rsplit_once(':')
            && let Ok(commander_id) = id_str.parse()
        {
            return Some(CallbackAction::ModuleSettings {
                module_key,
                rest,
                commander_id,
            });
        }

        return Some(CallbackAction::ModuleSettings {
            module_key,
            rest: full_rest,
            commander_id: 0,
        });
    }

    if let Some(commander_id_str) = data.strip_prefix("delete_data:")
        && let Ok(commander_id) = commander_id_str.parse()
    {
        return Some(CallbackAction::DeleteData { commander_id });
    }

    if data.starts_with("delete_data_confirm:") {
        return Some(CallbackAction::DeleteDataConfirmation);
    }
    if data.starts_with("delete_msg") {
        return Some(CallbackAction::DeleteMessage);
    }
    if data.starts_with("delete_confirm:") {
        return Some(CallbackAction::DeleteMessageConfirmation);
    }
    if data == "summarize" {
        return Some(CallbackAction::Summarize);
    }
    if data == "retell" {
        return Some(CallbackAction::Retell);
    }
    if let Some(rest) = data.strip_prefix("retry_speech:") {
        let parts: Vec<_> = rest.splitn(3, ':').collect();
        if parts.len() == 3
            && let Ok(message_id) = parts[0].parse()
            && let Ok(attempt) = parts[2].parse()
        {
            return Some(CallbackAction::RetrySpeech {
                message_id,
                action_type: parts[1],
                attempt,
            });
        }
    }
    if data.starts_with("speech:page:") || data.starts_with("summary:page:") {
        return Some(CallbackAction::TranscriptionPagination);
    }
    if data.starts_with("back_to_full") {
        return Some(CallbackAction::BackToFull);
    }
    if data.starts_with("whisper") {
        return Some(CallbackAction::Whisper);
    }
    if data.starts_with("tr_") || data.starts_with("tr:") {
        return Some(CallbackAction::Translate);
    }
    if data.starts_with("cobalt:") {
        return Some(CallbackAction::CobaltPagination);
    }

    if let Some(rest) = data.strip_prefix("help:page:") {
        // help:page:PAGE:USER_ID
        let parts: Vec<_> = rest.split(':').collect();
        if parts.len() == 2
            && let Ok(page) = parts[0].parse()
            && let Ok(user_id) = parts[1].parse()
        {
            return Some(CallbackAction::HelpPagination { page, user_id });
        }
    }

    if data.starts_with("admin:") {
        return Some(CallbackAction::Admin);
    }

    if let Some(rest) = data.strip_prefix("refactor:") {
        let parts: Vec<_> = rest.split(':').collect();
        if parts.len() == 2
            && let Ok(id) = parts[1].parse()
        {
            return Some(CallbackAction::Refactor {
                mode: parts[0],
                src_msg_id: id,
            });
        }
    }

    None
}

pub async fn callback_query_handlers(bot: Bot, q: CallbackQuery) -> Result<(), MyError> {
    let config = Arc::new(Config::new().await);

    let Some(data) = &q.data.clone() else {
        return Ok(());
    };

    let mut locale = get_user_locale(&q.from, &config).await;

    match parse_callback_data(data) {
        Some(CallbackAction::SettingsMain {
            owner_type,
            owner_id,
            commander_id,
        }) => {
            if q.from.id.0 != commander_id {
                bot.answer_callback_query(q.id)
                    .text(t!("errors.no_permission", locale = &locale))
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            locale = get_locale_by_owner(owner_id, owner_type, &config).await;

            let owner = Owner {
                id: owner_id.to_string(),
                r#type: owner_type.to_string(),
            };

            let settings = Settings::get_or_create(&owner).await?;
            if let Some(MaybeInaccessibleMessage::Regular(msg)) = &q.message {
                if (msg.chat.is_group() || msg.chat.is_supergroup()) && settings.admin_only_mode {
                    let member = bot.get_chat_member(msg.chat.id, q.from.id).await?;
                    if !member.is_privileged() {
                        bot.answer_callback_query(q.id)
                            .text(t!("errors.no_permission", locale = &locale))
                            .show_alert(true)
                            .await?;
                        return Ok(());
                    }
                }
            }

            let Some(MaybeInaccessibleMessage::Regular(msg)) = q.message else {
                return Ok(());
            };

            let (text, kb) = get_main_settings_menu(
                &locale,
                owner_type,
                owner_id,
                commander_id,
                settings.admin_only_mode,
            );
            bot.edit_message_text(msg.chat.id, msg.id, text)
                .reply_markup(kb)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }
        Some(CallbackAction::ToggleAdminOnly {
            owner_type,
            owner_id,
            commander_id,
        }) => {
            if q.from.id.0 != commander_id {
                bot.answer_callback_query(q.id)
                    .text(t!("errors.no_permission", locale = &locale))
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            if let Some(MaybeInaccessibleMessage::Regular(msg)) = &q.message {
                if msg.chat.is_group() || msg.chat.is_supergroup() {
                    let member = bot.get_chat_member(msg.chat.id, q.from.id).await?;
                    if !member.is_privileged() {
                        bot.answer_callback_query(q.id)
                            .text(t!("errors.no_permission", locale = &locale))
                            .show_alert(true)
                            .await?;
                        return Ok(());
                    }
                }
            }

            let owner = Owner {
                id: owner_id.to_string(),
                r#type: owner_type.to_string(),
            };

            let new_state = Settings::toggle_admin_mode(&owner).await?;
            locale = get_locale_by_owner(owner_id, owner_type, &config).await;

            let Some(MaybeInaccessibleMessage::Regular(msg)) = q.message else {
                return Ok(());
            };

            let (text, kb) =
                get_main_settings_menu(&locale, owner_type, owner_id, commander_id, new_state);
            bot.edit_message_text(msg.chat.id, msg.id, text)
                .reply_markup(kb)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }
        Some(CallbackAction::SettingsLang {
            owner_type,
            owner_id,
            commander_id,
        }) => {
            if q.from.id.0 != commander_id {
                bot.answer_callback_query(q.id)
                    .text(t!("errors.no_permission", locale = &locale))
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            locale = get_locale_by_owner(owner_id, owner_type, &config).await;

            let Some(MaybeInaccessibleMessage::Regular(msg)) = q.message else {
                return Ok(());
            };

            let (text, kb) = get_lang_settings_menu(&locale, owner_type, owner_id, commander_id);
            bot.edit_message_text(msg.chat.id, msg.id, text)
                .reply_markup(kb)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }
        Some(CallbackAction::LangSet {
            lang_code,
            owner_type,
            owner_id,
            commander_id,
        }) => {
            if q.from.id.0 != commander_id {
                bot.answer_callback_query(q.id)
                    .text(t!("errors.no_permission", locale = &locale))
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            set_locale(owner_id, owner_type, lang_code, &config).await?;
            locale = lang_code.to_string();

            bot.answer_callback_query(q.id)
                .text(t!("settings.lang_selected_alert", locale = &locale))
                .await?;

            let Some(MaybeInaccessibleMessage::Regular(msg)) = q.message else {
                return Ok(());
            };

            let owner = Owner {
                id: owner_id.to_string(),
                r#type: owner_type.to_string(),
            };
            let settings = Settings::get_or_create(&owner).await?;

            let (text, kb) = get_main_settings_menu(
                &locale,
                owner_type,
                owner_id,
                commander_id,
                settings.admin_only_mode,
            );
            bot.edit_message_text(msg.chat.id, msg.id, text)
                .reply_markup(kb)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }
        Some(CallbackAction::SettingsModules {
            owner_type,
            owner_id,
            commander_id,
        }) => {
            if q.from.id.0 != commander_id {
                bot.answer_callback_query(q.id)
                    .text(t!("errors.no_permission", locale = &locale))
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            locale = get_locale_by_owner(owner_id, owner_type, &config).await;

            let Some(MaybeInaccessibleMessage::Regular(msg)) = q.message else {
                return Ok(());
            };

            let (text, kb) =
                get_modules_settings_menu(&locale, owner_type, owner_id, commander_id).await?;
            bot.edit_message_text(msg.chat.id, msg.id, text)
                .reply_markup(kb)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }
        Some(CallbackAction::ModuleSelect {
            owner_type,
            owner_id,
            module_key,
            commander_id,
        }) => {
            if q.from.id.0 != commander_id {
                bot.answer_callback_query(q.id)
                    .text(t!("errors.no_permission", locale = &locale))
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            if let Some(module) = MOD_MANAGER.get_module(module_key) {
                let Some(MaybeInaccessibleMessage::Regular(msg)) = q.message else {
                    return Ok(());
                };

                let owner = Owner {
                    id: owner_id.to_string(),
                    r#type: owner_type.to_string(),
                };
                let (text, keyboard) = module.get_settings_ui(&owner, commander_id).await?;
                bot.edit_message_text(msg.chat.id, msg.id, text)
                    .reply_markup(keyboard)
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
            }
        }
        Some(CallbackAction::SettingsBack {
            owner_type,
            owner_id,
            commander_id,
        }) => {
            if q.from.id.0 != commander_id {
                bot.answer_callback_query(q.id)
                    .text(t!("errors.no_permission", locale = &locale))
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            locale = get_locale_by_owner(owner_id, owner_type, &config).await;

            let Some(MaybeInaccessibleMessage::Regular(message)) = q.message else {
                return Ok(());
            };

            let (text, kb) =
                get_modules_settings_menu(&locale, owner_type, owner_id, commander_id).await?;
            bot.edit_message_text(message.chat.id, message.id, text)
                .reply_markup(kb)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }
        Some(CallbackAction::ModuleSettings {
            module_key,
            rest,
            commander_id,
        }) => {
            if q.from.id.0 != commander_id && commander_id != 0 {
                bot.answer_callback_query(q.id)
                    .text(t!("errors.no_permission", locale = &locale))
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            if let Some(module) = MOD_MANAGER.get_module(module_key)
                && let Some(MaybeInaccessibleMessage::Regular(msg)) = &q.message
            {
                let owner = Owner {
                    id: msg.chat.id.to_string(),
                    r#type: (if msg.chat.is_private() {
                        "user"
                    } else {
                        "group"
                    })
                    .to_string(),
                };
                module
                    .handle_callback(bot, &q, &owner, rest, commander_id)
                    .await?;
            }
        }
        Some(CallbackAction::DeleteData { commander_id }) => {
            if q.from.id.0 != commander_id {
                bot.answer_callback_query(q.id)
                    .text(t!("errors.no_permission", locale = &locale))
                    .show_alert(true)
                    .await?;
                return Ok(());
            }
            handle_delete_data(bot, q, &config).await?
        }
        Some(CallbackAction::CobaltPagination) => handle_cobalt_pagination(bot, q, config).await?,
        Some(CallbackAction::DeleteDataConfirmation) => {
            handle_delete_data_confirmation(bot, q, &config).await?
        }
        Some(CallbackAction::DeleteMessage) => handle_delete_request(bot, q, &config).await?,
        Some(CallbackAction::DeleteMessageConfirmation) => {
            handle_delete_confirmation(bot, q, &config).await?
        }
        Some(CallbackAction::Summarize) => {
            bot.answer_callback_query(q.id.clone()).await?;
            let (bot_c, q_c, config_c) = (bot.clone(), q.clone(), config.clone());

            tokio::spawn(async move {
                if let Err(e) = summarization_handler(bot_c, q_c, &config_c).await {
                    error!("Error in summarization_handler: {:?}", e);
                }
            });
        }
        Some(CallbackAction::Retell) => {
            bot.answer_callback_query(q.id.clone()).await?;
            let (bot_c, q_c, config_c) = (bot.clone(), q.clone(), config.clone());

            tokio::spawn(async move {
                if let Err(e) = summarization_handler(bot_c, q_c, &config_c).await {
                    error!("Error in summarization_handler: {:?}", e);
                }
            });
        }
        Some(CallbackAction::RetrySpeech {
            message_id,
            action_type,
            attempt,
        }) => {
            bot.answer_callback_query(q.id.clone()).await?;
            let (bot_c, q_c, config_c, action_type) = (
                bot.clone(),
                q.clone(),
                config.clone(),
                action_type.to_string(),
            );

            tokio::spawn(async move {
                if let Err(e) =
                    retry_speech_handler(bot_c, q_c, &config_c, message_id, &action_type, attempt)
                        .await
                {
                    error!("Error in retry_speech_handler: {:?}", e);
                }
            });
        }
        Some(CallbackAction::TranscriptionPagination) => {
            pagination_handler(bot, q, &config).await?
        }
        Some(CallbackAction::BackToFull) => back_handler(bot, q, &config).await?,
        Some(CallbackAction::Whisper) => handle_whisper_callback(bot, q, &config).await?,
        Some(CallbackAction::Translate) => handle_translate_callback(bot, q, &config).await?,
        Some(CallbackAction::HelpPagination { page, user_id }) => {
            handle_help_pagination_callback(bot, q, &config, page, user_id).await?;
        }
        Some(CallbackAction::Admin) => {
            admin::handle_admin_callback(bot, q, &config).await?;
        }
        Some(CallbackAction::NoOp) => {
            bot.answer_callback_query(q.id).await?;
        }
        Some(CallbackAction::Refactor { mode, src_msg_id }) => {
            refactor::handle_refactor_callback(bot, q, &config, mode, src_msg_id).await?
        }
        None => {
            log::warn!("Unhandled callback query data: {}", data);
            bot.answer_callback_query(q.id).await?;
        }
    }

    Ok(())
}
