use crate::{
    bot::{
        keyboards::{delete::delete_message_button, translate::create_language_keyboard},
        modules::{Owner, translate::TranslateSettings},
    },
    core::{
        config::Config,
        db::schemas::settings::Settings,
        services::translation::{SUPPORTED_LANGUAGES, normalize_language_code},
    },
    errors::MyError,
    t,
    util::{
        i18n::get_chat_locale,
        paginator::{FrameBuild, Paginator},
    },
};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, Message, ParseMode, ReplyParameters},
    utils::html::escape,
};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TranslationCache {
    pub(crate) pages: Vec<String>,
    pub(crate) user_id: u64,
    pub(crate) original_url: Option<String>,
    pub(crate) target_lang: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TranslateJob {
    pub text: String,
    pub user_id: u64,
}

pub fn split_text_tr(text: &str, chunk_size: usize) -> Vec<String> {
    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start_idx = 0;

    while start_idx < text.len() {
        let remaining_len = text.len() - start_idx;

        if remaining_len <= chunk_size {
            chunks.push(text[start_idx..].trim().to_string());
            break;
        }

        let mut split_idx = start_idx + chunk_size;
        while !text.is_char_boundary(split_idx) {
            split_idx -= 1;
        }

        let slice = &text[start_idx..split_idx];

        let best_split = slice
            .rfind("\n\n")
            .map(|i| i + 2)
            .or_else(|| {
                slice
                    .rfind(". ")
                    .map(|i| i + 1)
                    .or_else(|| slice.rfind("! ").map(|i| i + 1))
                    .or_else(|| slice.rfind("? ").map(|i| i + 1))
            })
            .or_else(|| slice.rfind('\n').map(|i| i + 1))
            .or_else(|| slice.rfind(' ').map(|i| i + 1));

        if let Some(offset) = best_split {
            let actual_split = start_idx + offset;
            chunks.push(text[start_idx..actual_split].trim().to_string());
            start_idx = actual_split;
        } else {
            chunks.push(text[start_idx..split_idx].trim().to_string());
            start_idx = split_idx;
        }
    }
    chunks.into_iter().filter(|s| !s.is_empty()).collect()
}

pub async fn translate_handler(
    bot: Bot,
    msg: &Message,
    config: &Config,
    arg: String,
) -> Result<(), MyError> {
    let locale = get_chat_locale(&msg.chat, config).await;

    let replied_to_message = match msg.reply_to_message() {
        Some(message) => message,
        None => {
            bot.send_message(
                msg.chat.id,
                t!("errors.reply_to_translate", locale = &locale),
            )
            .reply_parameters(ReplyParameters::new(msg.id))
            .parse_mode(ParseMode::Html)
            .await?;
            return Ok(());
        }
    };

    let text_to_translate = match replied_to_message.text().or(replied_to_message.caption()) {
        Some(text) => text,
        None => {
            bot.send_message(msg.chat.id, t!("errors.reply_to_text", locale = &locale))
                .reply_parameters(ReplyParameters::new(msg.id))
                .parse_mode(ParseMode::Html)
                .await?;
            return Ok(());
        }
    };

    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };

    if let Some(author) = replied_to_message.from.as_ref()
        && author.is_bot
    {
        bot.send_message(msg.chat.id, t!("errors.reply_to_user", locale = &locale))
            .reply_parameters(ReplyParameters::new(msg.id))
            .parse_mode(ParseMode::Html)
            .await?;
        return Ok(());
    }

    let owner = Owner {
        id: msg.chat.id.to_string(),
        r#type: if msg.chat.is_private() {
            "user".to_string()
        } else {
            "group".to_string()
        },
    };
    let settings: TranslateSettings = Settings::get_module_settings(&owner, "translate").await?;

    if !settings.enabled {
        bot.send_message(msg.chat.id, t!("errors.module_disabled", locale = &locale))
            .await?;
        return Ok(());
    }

    let target_lang: String;

    if !arg.trim().is_empty() {
        target_lang = normalize_language_code(arg.trim());
    } else {
        let redis_key = format!("user_lang:{}", user.id);
        let redis_client = config.get_redis_client();
        let cached_lang: Option<String> = redis_client.get(&redis_key).await?;

        if let Some(lang) = cached_lang {
            target_lang = lang;
        } else {
            let job = TranslateJob {
                text: text_to_translate.to_string(),
                user_id: user.id.0,
            };

            config
                .get_redis_client()
                .set(&format!("translate_job:{}", user.id), &job, 600)
                .await?;

            let keyboard = create_language_keyboard(0, user.id.0);
            bot.send_message(msg.chat.id, t!("translate.select_lang", locale = &locale))
                .reply_markup(keyboard)
                .reply_parameters(ReplyParameters::new(replied_to_message.id))
                .await?;

            return Ok(());
        }
    }

    let text_chunks = split_text_tr(text_to_translate, 700);

    let translation_futures = text_chunks.iter().map(|chunk| {
        let target = target_lang.clone();
        async move {
            config
                .get_mozhi_client()
                .request(chunk, target)
                .engine(settings.default_engine)
                .source("auto")
                .send()
                .await
        }
    });

    let results = join_all(translation_futures).await;
    let translated_chunks: Vec<String> = results
        .into_iter()
        .filter_map(|res| match res {
            Ok(val) => Some(val),
            Err(e) => {
                log::error!("❌ Translation chunk failed: {:?}", e);
                None
            }
        })
        .collect();
    let full_translated_text = translated_chunks.join("\n\n");

    if full_translated_text.is_empty() {
        bot.send_message(msg.chat.id, t!("translate.failed", locale = &locale))
            .await?;
        return Ok(());
    }

    let display_pages = split_text_tr(&full_translated_text, 4000);

    let lang_display_name = SUPPORTED_LANGUAGES
        .iter()
        .find(|(code, _)| *code == target_lang)
        .map(|(_, name)| *name)
        .unwrap_or(&target_lang);

    if display_pages.len() <= 1 {
        let response = format!("<blockquote>{}</blockquote>", escape(&full_translated_text));

        let switch_lang_button =
            InlineKeyboardButton::callback(lang_display_name.to_string(), "tr_show_langs");

        let mut keyboard = delete_message_button(user.id.0);
        match keyboard.inline_keyboard.get_mut(0) {
            Some(first_row) => {
                first_row.insert(0, switch_lang_button);
            }
            None => {
                keyboard.inline_keyboard.push(vec![switch_lang_button]);
            }
        }

        bot.send_message(msg.chat.id, response)
            .reply_parameters(ReplyParameters::new(replied_to_message.id))
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await?;
    } else {
        let translation_id = Uuid::new_v4().to_string();
        let redis_key = format!("translation:{}", translation_id);

        let cache_data = TranslationCache {
            pages: display_pages.clone(),
            user_id: user.id.0,
            original_url: None,
            target_lang: target_lang.to_string(),
        };
        config
            .get_redis_client()
            .set(&redis_key, &cache_data, 3600)
            .await?;

        let switch_lang_button =
            InlineKeyboardButton::callback(lang_display_name.to_string(), "tr_show_langs");
        let delete_button = delete_message_button(user.id.0)
            .inline_keyboard
            .remove(0)
            .remove(0);

        let keyboard = Paginator::new("tr", display_pages.len())
            .current_page(0)
            .set_callback_formatter(move |page| format!("tr:page:{}:{}", translation_id, page))
            .add_bottom_row(vec![switch_lang_button, delete_button])
            .build();

        let response_text = format!("<blockquote>{}</blockquote>", escape(&display_pages[0]));
        bot.send_message(msg.chat.id, response_text)
            .reply_parameters(ReplyParameters::new(replied_to_message.id))
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .await?;
    }

    Ok(())
}
