use crate::{
    bot::modules::{Owner, translate::TranslateSettings},
    core::{
        config::Config,
        db::schemas::settings::Settings,
        services::translation::{SUPPORTED_LANGUAGES, normalize_language_code},
    },
    errors::MyError,
    t,
    util::i18n::get_user_locale,
};
use futures::future::join_all;
use std::sync::Arc;
use teloxide::{
    payloads::AnswerInlineQuerySetters,
    prelude::*,
    types::{
        InlineQuery, InlineQueryResult, InlineQueryResultArticle, InputMessageContent,
        InputMessageContentText,
    },
};
use translators::{GoogleTranslator, Translator};
use uuid::Uuid;

pub async fn handle_translate_inline(
    bot: Bot,
    q: InlineQuery,
    config: Arc<Config>,
) -> Result<(), MyError> {
    let locale = get_user_locale(&q.from, &config).await;
    let text_to_translate = q.query.trim();

    if text_to_translate.is_empty() {
        let help_article = InlineQueryResultArticle::new(
            "translate_help",
            t!("modules.translate.help_title", locale = &locale),
            InputMessageContent::Text(InputMessageContentText::new(t!(
                "modules.translate.help_text",
                locale = &locale
            ))),
        )
        .description(t!("translate.help_desc", locale = &locale));

        bot.answer_inline_query(q.id, vec![InlineQueryResult::Article(help_article)])
            .cache_time(10)
            .await?;
        return Ok(());
    }

    let redis_key = format!("user_lang:{}", q.from.id);
    let cached_lang: Option<String> = config.get_redis_client().get(&redis_key).await?;

    // TODO: impl custom target languages
    let mut target_langs = vec![
        "en".to_string(),
        "ru".to_string(),
        "uk".to_string(),
        "de".to_string(),
    ];
    if let Some(lang) = cached_lang {
        target_langs.retain(|l| l != &lang);
        target_langs.insert(0, lang);
    }
    target_langs.dedup();

    let google_trans = GoogleTranslator::default();
    let translation_futures = target_langs.iter().map(|lang| {
        let text = text_to_translate.to_string();
        let lang = lang.to_string();
        let value = google_trans.clone();
        async move {
            let normalized_lang = normalize_language_code(&lang);
            value
                .translate_async(&text, "", &normalized_lang)
                .await
                .map(|translated_text| (normalized_lang, translated_text))
        }
    });

    let results = join_all(translation_futures).await;
    let successful_translations: Vec<(String, String)> =
        results.into_iter().filter_map(Result::ok).collect();

    if successful_translations.is_empty() {
        let no_result_article = InlineQueryResultArticle::new(
            "translate_no_result",
            t!("translate.failed_title", locale = &locale),
            InputMessageContent::Text(InputMessageContentText::new(t!(
                "translate.failed",
                locale = &locale
            ))),
        )
        .description(t!("translate.failed_desc", locale = &locale));

        bot.answer_inline_query(q.id, vec![InlineQueryResult::Article(no_result_article)])
            .await?;
        return Ok(());
    }

    let mut articles = Vec::new();
    for (lang_code, translated_text) in successful_translations {
        let lang_display_name = SUPPORTED_LANGUAGES
            .iter()
            .find(|(code, _)| *code == lang_code)
            .map(|(_, name)| *name)
            .unwrap_or(&lang_code);

        let article = InlineQueryResultArticle::new(
            Uuid::new_v4().to_string(),
            t!(
                "translate.result_title",
                locale = &locale,
                lang = lang_display_name
            ),
            InputMessageContent::Text(InputMessageContentText::new(translated_text.clone())),
        )
        .description(translated_text);
        articles.push(InlineQueryResult::Article(article));
    }

    bot.answer_inline_query(q.id, articles).await?;

    Ok(())
}

pub async fn is_translate_query(q: InlineQuery) -> bool {
    let owner = Owner {
        id: q.from.id.to_string(),
        r#type: "user".to_string(),
    };

    let settings = Settings::get_module_settings::<TranslateSettings>(&owner, "translate")
        .await
        .unwrap();
    settings.enabled
}
