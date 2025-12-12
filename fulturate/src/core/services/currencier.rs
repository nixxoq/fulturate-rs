use crate::{
    core::{
        db::{
            functions::get_or_create,
            schemas::{BaseFunctions, CurrenciesFunctions, group::Group, user::User},
        },
        services::currency::converter::{CURRENCY_CONFIG_PATH, get_all_currency_codes},
    },
    errors::MyError,
};
use log::error;
use std::collections::HashSet;
use teloxide::{
    prelude::*,
    types::{ParseMode, ReplyParameters},
};

pub async fn handle_currency_update<T>(bot: Bot, msg: Message, code: String) -> Result<(), MyError>
where
    T: BaseFunctions + CurrenciesFunctions + Send + Sync,
{
    let code = code.to_uppercase();

    let all_codes = get_all_currency_codes(CURRENCY_CONFIG_PATH.parse().unwrap())?;
    let currency = all_codes.iter().find(|c| c.code == code);

    if currency.is_none() {
        bot.send_message(
            msg.chat.id,
            format!("Currency code <code>{}</code> does not exist.", code),
        )
        .parse_mode(ParseMode::Html)
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;
        return Ok(());
    }

    let entity = match get_or_create::<T>(msg.chat.id.to_string()).await {
        Ok(e) => e,
        Err(e) => {
            error!(
                "Failed to get or create entity in chat {}: {:?}",
                msg.chat.id, e
            );
            bot.send_message(
                msg.chat.id,
                "Error: Could not access settings. Try again later.",
            )
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
            return Ok(());
        }
    };

    let is_enabled = entity.get_currencies().iter().any(|c| c.code == code);
    let key = entity.get_key();

    let result = if is_enabled {
        T::remove_currency(key, &code).await.map(|_| "removed")
    } else {
        T::add_currency(key, currency.unwrap())
            .await
            .map(|_| "added")
    };

    let message = match result {
        Ok(action) => format!(
            "Successfully {} <code>{}</code> from currency conversion.",
            action, code
        ),
        Err(e) => {
            error!("Failed to update currency for {}: {:?}", msg.chat.id, e);
            "Failed to apply changes.".to_string()
        }
    };

    bot.send_message(msg.chat.id, message)
        .parse_mode(ParseMode::Html)
        .reply_parameters(ReplyParameters::new(msg.id))
        .await?;

    Ok(())
}

pub async fn get_enabled_codes(msg: &Message) -> HashSet<String> {
    let chat_id = msg.chat.id.to_string();

    async fn fetch<T: BaseFunctions + CurrenciesFunctions>(id: String) -> HashSet<String> {
        if let Ok(Some(entity)) = T::get(id).await {
            entity
                .get_currencies()
                .iter()
                .map(|c| c.code.clone())
                .collect()
        } else {
            HashSet::new()
        }
    }

    if msg.chat.is_private() {
        fetch::<User>(chat_id).await
    } else {
        fetch::<Group>(chat_id).await
    }
}
