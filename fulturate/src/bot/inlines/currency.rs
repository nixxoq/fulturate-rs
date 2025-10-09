use crate::{
    bot::modules::{Owner, currency::CurrencySettings},
    core::{
        config::Config,
        db::schemas::settings::Settings,
    },
    errors::MyError,
};
use log::{debug, error};
use std::sync::Arc;
use teloxide::{
    prelude::*,
    types::{
        InlineQuery, InlineQueryResult, InlineQueryResultArticle, InputMessageContent,
        InputMessageContentText, Me, ParseMode,
    },
    utils::html::escape,
};
use uuid::Uuid;

pub async fn is_currency_query(q: InlineQuery) -> bool {
    if !q.query.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }

    let owner = Owner {
        id: q.from.id.to_string(),
        r#type: "user".to_string(),
    };

    match Settings::get_module_settings::<CurrencySettings>(&owner, "currency").await {
        Ok(settings) => settings.enabled,
        Err(e) => {
            error!(
                "DB error checking currency module status for user {}: {}",
                q.from.id, e
            );
            false
        }
    }
}

pub async fn handle_currency_inline(
    bot: Bot,
    q: InlineQuery,
    config: Arc<Config>,
    _me: Me,
) -> Result<(), MyError> {
    debug!("Handling currency inline query: {}", &q.query);

    let owner = Owner {
        id: q.from.id.to_string(),
        r#type: "user".to_string(),
    };

    let converter = config.get_currency_converter();
    match converter.process_text(&q.query, &owner).await {
        Ok(mut results) if !results.is_empty() => {
            results.truncate(5);

            let raw_results = results.join("\n");

            let formatted_blocks: Vec<String> = results
                .into_iter()
                .map(|result_block| format!("<blockquote expandable>{}</blockquote>", escape(&result_block)))
                .collect();

            let final_message = formatted_blocks.join("\n");

            let article = InlineQueryResultArticle::new(
                Uuid::new_v4().to_string(),
                "Результат конвертации",
                InputMessageContent::Text(
                    InputMessageContentText::new(final_message.clone()).parse_mode(ParseMode::Html),
                ),
            )
                .description(raw_results);

            if let Err(e) = bot
                .answer_inline_query(q.id, vec![InlineQueryResult::Article(article)])
                .cache_time(2)
                .await
            {
                error!("Failed to answer currency inline query: {:?}", e);
            }
        }
        Err(e) => {
            error!(
                "Currency conversion processing error in inline mode: {:?}",
                e
            );
        }
        _ => {
            debug!("No currency conversion results for: {}", &q.query);
        }
    }

    Ok(())
}