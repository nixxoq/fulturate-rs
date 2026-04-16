use crate::{
    core::config::Config,
    core::services::cobalt::{InstanceData, get_cobalt_status, get_total_updates},
    errors::MyError,
    // util::i18n::get_chat_locale,
};
use std::fmt::Write;
use teloxide::{prelude::*, sugar::request::RequestReplyExt, types::ParseMode};

pub async fn stats_handler(bot: Bot, msg: Message, config: &Config) -> Result<(), MyError> {
    let status_msg = bot
        .send_message(msg.chat.id, "⌛ <i>Gathering statistics...</i>")
        .parse_mode(ParseMode::Html)
        .reply_to(msg.id)
        .await?;

    let status_data = get_cobalt_status(config).await.ok().flatten();
    let total_updates = get_total_updates();

    let response_text = format_stats_message(config.get_version(), total_updates, status_data);

    bot.edit_message_text(msg.chat.id, status_msg.id, response_text)
        .parse_mode(ParseMode::Html)
        .await?;

    Ok(())
}

fn format_stats_message(version: &str, updates: u64, status: Option<InstanceData>) -> String {
    let mut f = String::new();

    let _ = writeln!(f, "📊");
    let _ = writeln!(f, "Updates processed: <code>{updates}</code>");
    let _ = writeln!(f, "Version: <code>{version}</code>");
    let _ = writeln!(f);

    match status {
        Some(instance) => {
            let _ = writeln!(f, "<b>Cobalt Instance:</b> <code>{}</code>", instance.api);
            f.push_str("<blockquote expandable>");

            let mut tests: Vec<_> = instance
                .tests
                .into_iter()
                .filter(|(k, _)| k != "Frontend")
                .collect();

            tests.sort_by(|a, b| a.0.cmp(&b.0));

            for (key, res) in tests {
                let name = res.friendly.as_deref().unwrap_or(&key);
                let icon = if res.status { "✅" } else { "❌" };
                let _ = writeln!(f, "{name} - {icon}");
            }

            f.push_str("</blockquote>");
        }
        None => {
            f.push_str("⚠️ <i>Cobalt instance status unavailable.</i>");
        }
    }

    f
}
