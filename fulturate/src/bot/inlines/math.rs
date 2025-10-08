use crate::bot::modules::math::MathSettings;
use crate::{
    bot::modules::Owner,
    core::{
        config::Config,
        db::schemas::settings::Settings
        ,
    },
    errors::MyError,
};
use eidolon_lang::interpreter::evaluate;
use std::collections::HashMap;
use std::sync::Arc;
use teloxide::{
    payloads::AnswerInlineQuerySetters,
    prelude::*,
    types::{
        InlineQuery, InlineQueryResult, InlineQueryResultArticle, InputMessageContent,
        InputMessageContentText,
    },
};

pub async fn handle_math_inline(
    bot: Bot,
    q: InlineQuery,
    config: Arc<Config>,
) -> Result<(), MyError> {
    let expression = q.query.trim();

    if expression.is_empty() {
        let help_article = InlineQueryResultArticle::new(
            "math_help",
            "Как использовать математическое выражение?",
            InputMessageContent::Text(InputMessageContentText::new(
                "Просто начните вводить пример, который хотите посчитать.",
            )),
        )
            .description("Введите текст для подсчёта...");

        bot.answer_inline_query(q.id, vec![InlineQueryResult::Article(help_article)])
            .cache_time(10)
            .await?;
        return Ok(());
    }

    let article;

    match eidolon_lang::parse_eidolon_source(expression) {
        Ok(ast) => {
            match evaluate(&ast, &HashMap::new()) {
                Ok(result) => {
                    article = InlineQueryResultArticle::new(
                        "eidolonia".to_string(),
                        format!("Результат: {}", result),
                        InputMessageContent::Text(InputMessageContentText::new(format!(
                            "Результат вычисления: {}", result
                        )))
                    );
                }
                Err(e) => {
                    article = InlineQueryResultArticle::new(
                        "eidolonia".to_string(),
                        format!("{}", e.message),
                        InputMessageContent::Text(InputMessageContentText::new(format!(
                            "Произошла ошибка при вычислении: {}", e.message
                        )))
                    );
                }
            }
        }
        Err(e) => {
            article = InlineQueryResultArticle::new(
                "eidolonia".to_string(),
                format!("{}", e.message),
                InputMessageContent::Text(InputMessageContentText::new(format!(
                    "Произошла ошибка при вычислении: {}", e.message
                )))
            );
        }
    }

    bot.answer_inline_query(q.id, vec![InlineQueryResult::Article(article)])
        .await?;

    Ok(())
}

pub async fn is_math_query(q: InlineQuery) -> bool {
    let owner = Owner {
        id: q.from.id.to_string(),
        r#type: "user".to_string(),
    };

    let settings = Settings::get_module_settings::<MathSettings>(&owner, "math").await.unwrap();
    settings.enabled
}
