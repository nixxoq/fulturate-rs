use crate::{
    core::config::Config,
    errors::MyError,
    t,
    util::{
        enums::Command,
        i18n::get_chat_locale,
        paginator::{FrameBuild, Paginator},
    },
};
use std::fmt::Write;
use teloxide::{
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode, ReplyParameters},
    utils::command::BotCommands,
};

pub async fn help_handler(
    bot: Bot,
    message: Message,
    config: &Config,
    _arg: String,
) -> Result<(), MyError> {
    let locale = get_chat_locale(&message.chat, config).await;

    let user_id = message.from.as_ref().map(|u| u.id.0).unwrap_or(0);
    let text = generate_help_text(&locale, 0);
    let keyboard = generate_help_keyboard(0, message.chat.id.0, user_id, &locale);

    bot.send_message(message.chat.id, text)
        .reply_parameters(ReplyParameters::new(message.id))
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

pub fn generate_help_text(locale: &str, page: usize) -> String {
    let mut text = String::with_capacity(1024);

    match page {
        0 => {
            let _ = write!(
                text,
                "{}<blockquote>",
                t!("help.header_commands", locale = locale)
            );
            for command in Command::bot_commands() {
                let name = command
                    .command
                    .strip_prefix('/')
                    .unwrap_or(&command.command);

                let key = format!("commands.{}.desc", name);
                let translated = t!(&key, locale = locale);

                let description = if translated == key {
                    command.description.as_str()
                } else {
                    &translated
                };

                let _ = writeln!(text, "/{} — {}", name, description);
            }
            text.push_str("</blockquote>");
        }
        _ => {
            let _ = write!(
                text,
                "{}<blockquote>",
                t!("help.header_inline", locale = locale)
            );
            let guides = [
                "help.guide_cobalt",
                "help.guide_currency",
                "help.guide_whisper",
                "help.guide_translate",
            ];
            for key in guides {
                let _ = writeln!(text, "{}", t!(key, locale = locale));
            }
            text.push_str("</blockquote>");
        }
    }

    text
}

pub fn generate_help_keyboard(
    page: usize,
    chat_id: i64,
    user_id: u64,
    locale: &str,
) -> InlineKeyboardMarkup {
    let total_pages = 2;

    let settings_data = format!("settings_main:user:{}:{}", chat_id, user_id);
    let settings_btn = InlineKeyboardButton::callback(
        t!("start.btn_settings_shortcut", locale = locale),
        settings_data,
    );

    Paginator::new("help", total_pages)
        .current_page(page)
        .set_callback_formatter(move |p| format!("help:page:{}:{}", p, user_id))
        .add_bottom_row(vec![settings_btn])
        .build()
}

pub async fn handle_help_pagination_callback(
    bot: Bot,
    q: CallbackQuery,
    config: &Config,
    page: usize,
    target_user_id: u64,
) -> Result<(), MyError> {
    let Some(message) = q.message.as_ref().and_then(|m| m.regular_message()) else {
        bot.answer_callback_query(q.id)
            .text("⚠️ Message expired")
            .await?;
        return Ok(());
    };

    let locale = get_chat_locale(&message.chat, config).await;

    if target_user_id != 0 && q.from.id.0 != target_user_id {
        bot.answer_callback_query(q.id)
            .text(t!("errors.no_permission", locale = &locale))
            .show_alert(true)
            .await?;
        return Ok(());
    }

    let text = generate_help_text(&locale, page);

    let keyboard = generate_help_keyboard(page, message.chat.id.0, target_user_id, &locale);

    bot.edit_message_text(message.chat.id, message.id, text)
        .reply_markup(keyboard)
        .parse_mode(ParseMode::Html)
        .await?;

    bot.answer_callback_query(q.id).await?;

    Ok(())
}
