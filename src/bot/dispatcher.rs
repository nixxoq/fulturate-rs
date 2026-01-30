// use crate::bot::inlines::math::{handle_math_inline, is_math_query};
use crate::{
    bot::{
        callbacks::callback_query_handlers,
        commander::command_handlers,
        inlines::{
            cobalter::{handle_cobalt_inline, handle_inline_video, is_query_url},
            currency::{handle_currency_inline, is_currency_query},
            translate::{handle_translate_inline, is_translate_query},
            whisper::{handle_whisper_inline, is_whisper_query},
        },
        keyboards::delete::delete_message_button_no_confirm,
        messager::{handle_currency, handle_speech},
        messages::chat::handle_bot_added,
        modules::{Owner, registry::MOD_MANAGER},
    },
    core::{
        config::Config,
        db::schemas::{settings::Settings, user::User as DBUser},
        metrics::{ERRORS_COUNTER, INCOMING_UPDATES},
    },
    errors::MyError,
    t,
    util::{enums::Command, i18n::get_locale_by_id, is_user_subscribed},
};
use log::{error, info};
use mongodb::bson::doc;
use oximod::{Model, OxiClient};
use serde::Deserialize;
use std::{convert::Infallible, fmt::Write, ops::ControlFlow, sync::Arc};
use teloxide::{
    Bot,
    dispatching::{Dispatcher, DpHandlerDescription, MessageFilterExt, UpdateFilterExt},
    dptree,
    error_handlers::LoggingErrorHandler,
    payloads::{
        AnswerInlineQuerySetters, DeleteWebhookSetters, SendDocumentSetters, SendMessageSetters,
    },
    prelude::{ChatId, Handler, Message, Requester},
    types::{
        InlineKeyboardButton, InlineKeyboardMarkup, InlineQuery, InlineQueryResult,
        InlineQueryResultArticle, InlineQueryResultsButton, InlineQueryResultsButtonKind,
        InputFile, InputMessageContent, InputMessageContentText, Me, MessageId, ParseMode,
        Recipient, ThreadId, Update,
    },
    update_listeners::Polling,
    utils::{command::BotCommands, html},
};

async fn root_handler(
    update: Update,
    config: Arc<Config>,
    bot: Bot,
    logic: Arc<Handler<'static, Result<(), MyError>, DpHandlerDescription>>,
    me: Me,
) -> Result<(), Infallible> {
    INCOMING_UPDATES.inc();

    let deps = dptree::deps![update.clone(), config.clone(), bot.clone(), me.clone()];
    let result = logic.dispatch(deps).await;

    if let ControlFlow::Break(Err(err)) = result {
        let error_handler_endpoint: Handler<'static, (), DpHandlerDescription> =
            dptree::endpoint(handle_error);
        let error_deps = dptree::deps![Arc::new(err), update, config, bot];
        let _ = error_handler_endpoint.dispatch(error_deps).await;
    }

    Ok(())
}

async fn subscription_guard(bot: Bot, msg: Message, config: Arc<Config>) -> Result<(), MyError> {
    let locale = get_locale_by_id(msg.from.unwrap().id.0, &config).await;
    let channel_url = format!(
        "https://t.me/{}",
        config.get_channel_username().replace("@", "")
    );

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
        t!("chat.subscribe_needed_btn", locale = &locale),
        channel_url.parse()?,
    )]]);

    bot.send_message(msg.chat.id, t!("chat.must_subscribe", locale = &locale))
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

async fn is_user_registered(q: InlineQuery) -> bool {
    let user_id_str = q.from.id.to_string();
    DBUser::find_one(doc! { "user_id": &user_id_str })
        .await
        .is_ok_and(|user| user.is_some())
}

async fn prompt_registration(
    bot: Bot,
    q: InlineQuery,
    me: Me,
    config: Arc<Config>,
) -> Result<(), MyError> {
    let locale = get_locale_by_id(q.from.id.0, &config).await;

    let start_url = format!("https://t.me/{}?start=inl", me.username());

    let keyboard = InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::url(
        t!("inline.btn_register", locale = &locale),
        start_url.parse()?,
    )]]);

    let article = InlineQueryResultArticle::new(
        "register_prompt",
        t!("inline.not_registered_title", locale = &locale),
        InputMessageContent::Text(InputMessageContentText::new(t!(
            "inline.not_registered_text",
            locale = &locale
        ))),
    )
    .description(t!("inline.not_registered_text", locale = &locale))
    .reply_markup(keyboard);

    if let Err(e) = bot
        .answer_inline_query(q.id, vec![InlineQueryResult::Article(article)])
        .cache_time(10)
        .await
    {
        error!("Failed to send 'register' inline prompt: {:?}", e);
    }

    Ok(())
}

#[derive(Deserialize)]
struct EnabledCheck {
    enabled: bool,
}

async fn are_any_inline_modules_enabled(q: InlineQuery) -> bool {
    let owner = Owner {
        id: q.from.id.to_string(),
        r#type: "user".to_string(),
    };

    if let Ok(settings) = Settings::get_or_create(&owner).await {
        for module in MOD_MANAGER.get_all_modules() {
            if module.is_enabled(&owner).await
                && let Some(settings_json) = settings.modules.get(module.key())
                && let Ok(check) = serde_json::from_value::<EnabledCheck>(settings_json.clone())
                && check.enabled
            {
                return true;
            }
        }
    }
    false
}

async fn send_modules_disabled_message(
    bot: Bot,
    q: InlineQuery,
    config: Arc<Config>,
) -> Result<(), MyError> {
    let locale = get_locale_by_id(q.from.id.0, &config).await;

    let article = InlineQueryResultArticle::new(
        "modules_disabled",
        t!("inline.modules_disabled_title", locale = &locale),
        InputMessageContent::Text(InputMessageContentText::new(t!(
            "inline.modules_disabled_text",
            locale = &locale
        ))),
    )
    .description(t!("inline.modules_disabled_text", locale = &locale));

    bot.answer_inline_query(q.id, vec![InlineQueryResult::Article(article)])
        .cache_time(10)
        .await?;
    Ok(())
}

async fn prompt_subscription_inline(
    bot: Bot,
    q: InlineQuery,
    config: Arc<Config>,
) -> Result<(), MyError> {
    let locale = get_locale_by_id(q.from.id.0, &config).await;

    bot.answer_inline_query(q.id, vec![])
        .button(InlineQueryResultsButton {
            text: t!("chat.subscribe_needed_btn", locale = &locale),
            kind: InlineQueryResultsButtonKind::StartParameter("register".to_string()),
        })
        .cache_time(0)
        .await?;

    Ok(())
}

pub fn inline_query_handler() -> Handler<'static, Result<(), MyError>, DpHandlerDescription> {
    dptree::entry()
        .branch(
            dptree::filter_async(|bot: Bot, q: InlineQuery, config: Arc<Config>| async move {
                !is_user_subscribed(
                    &bot,
                    q.from.id,
                    Recipient::ChannelUsername(config.get_channel_username().to_string()),
                )
                .await
            })
            .endpoint(prompt_subscription_inline),
        )
        .branch(
            dptree::filter_async(|q: InlineQuery| async move { !is_user_registered(q).await })
                .endpoint(prompt_registration),
        )
        .branch(
            dptree::filter_async(is_user_registered)
                .filter_async(
                    |q: InlineQuery| async move { !are_any_inline_modules_enabled(q).await },
                )
                .endpoint(send_modules_disabled_message),
        )
        .branch(
            dptree::filter_async(is_user_registered)
                .filter_async(are_any_inline_modules_enabled)
                .branch(dptree::filter_async(is_query_url).endpoint(handle_cobalt_inline))
                .branch(dptree::filter_async(is_currency_query).endpoint(handle_currency_inline))
                // .branch(dptree::filter_async(is_math_query).endpoint(handle_math_inline))
                .branch(dptree::filter_async(is_translate_query).endpoint(handle_translate_inline))
                .branch(dptree::filter_async(is_whisper_query).endpoint(handle_whisper_inline)),
        )
}

async fn run_bot(config: Arc<Config>) -> Result<(), MyError> {
    let command_menu = Command::bot_commands();
    let bot = config.get_bot();
    bot.delete_webhook().drop_pending_updates(true).await?;
    bot.set_my_commands(command_menu.clone()).await?;

    let sub_check =
        dptree::filter_async(|bot: Bot, msg: Message, config: Arc<Config>| async move {
            if let Some(user) = msg.from {
                return is_user_subscribed(
                    &bot,
                    user.id,
                    Recipient::ChannelUsername(config.get_channel_username().to_string()),
                )
                .await;
            }
            false
        });

    let logic_handlers = dptree::entry()
        .branch(
            Update::filter_message()
                .branch(
                    dptree::filter(|msg: Message| msg.chat.is_private())
                        .branch(
                            sub_check
                                .branch(
                                    teloxide::filter_command::<Command, _>()
                                        .endpoint(command_handlers),
                                )
                                .branch(Message::filter_text().endpoint(handle_currency))
                                .branch(Message::filter_video_note().endpoint(handle_speech))
                                .branch(Message::filter_voice().endpoint(handle_speech)),
                        )
                        .endpoint(subscription_guard),
                )
                .branch(
                    dptree::filter(|msg: Message| !msg.chat.is_private())
                        .branch(teloxide::filter_command::<Command, _>().endpoint(command_handlers))
                        .branch(Message::filter_text().endpoint(handle_currency))
                        .branch(Message::filter_video_note().endpoint(handle_speech))
                        .branch(Message::filter_voice().endpoint(handle_speech)),
                ),
        )
        .branch(Update::filter_callback_query().endpoint(callback_query_handlers))
        .branch(Update::filter_my_chat_member().endpoint(handle_bot_added))
        .branch(Update::filter_inline_query().branch(inline_query_handler()))
        .branch(Update::filter_chosen_inline_result().endpoint(handle_inline_video));

    let me = bot.get_me().await?;
    info!("Bot name: {:?}", me.username());

    let listener = Polling::builder(bot.clone()).drop_pending_updates().build();

    Dispatcher::builder(bot.clone(), dptree::endpoint(root_handler))
        .dependencies(dptree::deps![config.clone(), Arc::new(logic_handlers), me])
        .enable_ctrlc_handler()
        .build()
        .dispatch_with_listener(listener, LoggingErrorHandler::new())
        .await;

    Ok(())
}

async fn run_database(config: Arc<Config>) -> Result<(), MyError> {
    let url = config.get_mongodb_url().to_owned();
    OxiClient::init_global(url.clone()).await?;
    info!("Database connected successfully. URL: {}", url);
    Ok(())
}

pub async fn run() -> Result<(), MyError> {
    let config = Arc::new(Config::new().await);
    let _th = tokio::join!(run_database(config.clone()), run_bot(config.clone()));
    Ok(())
}

pub async fn handle_error(err: Arc<MyError>, update: Update, config: Arc<Config>, bot: Bot) {
    if format!("{:?}", err).contains("query is too old") {
        return;
    }

    ERRORS_COUNTER.with_label_values(&["generic_error"]).inc();

    error!("Error: {:#}", err);

    let mut file_content = String::new();
    writeln!(&mut file_content, "Error chain").unwrap();
    for (i, cause) in err.chain().enumerate() {
        writeln!(&mut file_content, "{}. {}", i, cause).unwrap();
    }

    writeln!(&mut file_content, "\nDebug info:").unwrap();
    writeln!(&mut file_content, "{:#?}", err).unwrap();

    writeln!(&mut file_content, "\nUpdate context (from teloxide)").unwrap();
    writeln!(&mut file_content, "{:#?}", update).unwrap();

    let document = InputFile::memory(file_content.into_bytes()).file_name("crash_report.txt");

    let message_text = format!(
        "🚨 <b>Ошибка бота</b>\n\nПричина: <code>{}</code>\n\n#error",
        html::escape(&err.to_string())
    );

    if let (Ok(chat_id), Ok(thread_id)) = (
        config.get_log_chat_id().parse::<i64>(),
        config.get_error_chat_thread_id().parse::<i32>(),
    ) {
        let _ = bot
            .send_document(ChatId(chat_id), document)
            .caption(message_text)
            .parse_mode(ParseMode::Html)
            .message_thread_id(ThreadId(MessageId(thread_id)))
            .reply_markup(delete_message_button_no_confirm(72))
            .await;
    } else {
        error!("Config error: invalid LOG_CHAT_ID or ERROR_CHAT_THREAD_ID");
    }
}
