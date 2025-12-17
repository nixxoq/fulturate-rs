use crate::{
    bot::modules::Owner,
    core::{config::Config, db::schemas::settings::Settings},
};
use mongodb::bson::doc;
use oximod::ModelTrait;
use teloxide::types::{User, UserId};

pub const DEFAULT_LOCALE: &str = "en";

pub fn is_supported(lang: &str) -> bool {
    matches!(lang, "en" | "ru" | "uk")
}

pub fn normalize_lang_code(code: Option<&str>) -> String {
    code.and_then(|c| c.split('-').next())
        .filter(|c| is_supported(c))
        .unwrap_or(DEFAULT_LOCALE)
        .to_string()
}

pub async fn get_locale_by_id(user_id: u64, config: &Config) -> String {
    let dummy_user = User {
        id: UserId(user_id),
        is_bot: false,
        first_name: "".to_string(),
        last_name: None,
        username: None,
        language_code: None,
        is_premium: false,
        added_to_attachment_menu: false,
    };
    get_user_locale(&dummy_user, config).await
}

pub async fn get_user_locale(user: &User, config: &Config) -> String {
    let redis_key = format!("user_locale:{}", user.id);
    let redis = config.get_redis_client();

    if let Ok(Some(cached_lang)) = redis.get::<String>(&redis_key).await {
        return cached_lang;
    }

    let owner = Owner {
        id: user.id.0.to_string(),
        r#type: "user".to_string(),
    };

    let db_lang = match Settings::get_or_create(&owner).await {
        Ok(settings) => settings.language,
        Err(_) => String::new(),
    };

    let final_lang = if is_supported(&db_lang) {
        db_lang
    } else {
        normalize_lang_code(user.language_code.as_deref())
    };

    let _ = redis.set(&redis_key, &final_lang, 3600 * 24).await;

    final_lang
}

pub async fn set_user_locale(
    user_id: u64,
    new_lang: &str,
    config: &Config,
) -> Result<(), crate::errors::MyError> {
    if !is_supported(new_lang) {
        return Ok(());
    }

    let owner_id = user_id.to_string();
    Settings::update_one(
        doc! { "owner_id": &owner_id, "owner_type": "user" },
        doc! { "$set": { "language": new_lang } },
    )
    .await?;

    let redis_key = format!("user_locale:{}", user_id);
    config
        .get_redis_client()
        .set(&redis_key, &new_lang.to_string(), 3600 * 24)
        .await?;

    Ok(())
}
