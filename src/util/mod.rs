use teloxide::Bot;
use teloxide::prelude::{ChatId, Requester, UserId};
use teloxide::types::{ChatMemberKind, Recipient, User};

pub mod currency_values;
pub mod enums;
pub mod i18n;
pub mod paginator;

pub const MAX_DURATION_SECONDS: u32 = 10 * 60;

pub fn split_text(text: &str, chunk_size: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }
    text.chars()
        .collect::<Vec<_>>()
        .chunks(chunk_size)
        .map(|c| c.iter().collect())
        .collect()
}

pub async fn is_admin_or_author(
    bot: &Bot,
    chat_id: ChatId,
    is_group: bool,
    clicker: &User,
    target_user_id: u64,
) -> bool {
    if target_user_id == 72 || clicker.id.0 == target_user_id {
        return true;
    }

    if is_group && let Ok(member) = bot.get_chat_member(chat_id, clicker.id).await {
        return member.is_privileged();
    }

    false
}

pub fn is_author(clicker: &User, target_user_id: u64) -> bool {
    if target_user_id == 72 || clicker.id.0 == target_user_id {
        return true;
    }

    false
}

pub async fn is_user_subscribed(bot: &Bot, user_id: UserId, chat: impl Into<Recipient>) -> bool {
    bot.get_chat_member(chat, user_id)
        .await
        .is_ok_and(|member| {
            !matches!(
                member.kind,
                ChatMemberKind::Left | ChatMemberKind::Banned(..)
            )
        })
}
