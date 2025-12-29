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

    let text = generate_help_text(&locale, 0);
    let keyboard = generate_help_keyboard(
        0,
        message.chat.id.0,
        message.from.as_ref().map(|u| u.id.0).unwrap_or(0),
        &locale,
    );

    bot.send_message(message.chat.id, text)
        .reply_parameters(ReplyParameters::new(message.id))
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

pub fn generate_help_text(locale: &str, page: usize) -> String {
    if page == 0 {
        let mut text = t!("help.header_commands", locale = locale);
        text.push_str("<blockquote>");

        for command in Command::bot_commands() {
            let command_name = if command.command.starts_with("/") {
                command.command[1..].to_string()
            } else {
                command.command.to_string()
            };

            let key = format!("commands.{}.desc", command_name);
            let description = t!(&key, locale = locale);
            let final_desc = if description == key {
                command.description
            } else {
                description
            };

            text.push_str(&format!("/{} — {}\n", command_name, final_desc));
        }

        text.push_str("</blockquote>");
        text
    } else {
        let mut text = t!("help.header_inline", locale = locale);
        text.push_str("<blockquote>");
        text.push_str(&t!("help.guide_cobalt", locale = locale));
        text.push('\n');
        text.push_str(&t!("help.guide_currency", locale = locale));
        text.push('\n');
        text.push_str(&t!("help.guide_whisper", locale = locale));
        text.push('\n');
        text.push_str(&t!("help.guide_translate", locale = locale));

        text.push_str("</blockquote>");
        text
    }
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
    let locale = get_chat_locale(q.message.as_ref().expect("msg").chat(), config).await;

    if q.from.id.0 != target_user_id && target_user_id != 0 {
        bot.answer_callback_query(q.id)
            .text(t!("errors.no_permission", locale = &locale))
            .show_alert(true)
            .await?;
        return Ok(());
    }

    let text = generate_help_text(&locale, page);

    let chat_id = q.message.as_ref().map(|m| m.chat().id.0).unwrap_or(0);
    let keyboard = generate_help_keyboard(page, chat_id, target_user_id, &locale);

    bot.edit_message_text(
        q.message.as_ref().expect("msg").chat().id,
        q.message.as_ref().expect("msg").id(),
        text,
    )
    .reply_markup(keyboard)
    .parse_mode(ParseMode::Html)
    .await?;

    bot.answer_callback_query(q.id).await?;

    Ok(())
}
