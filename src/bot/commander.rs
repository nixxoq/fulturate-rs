use crate::{
    bot::commands::{
        admin::{admin_command_handler, broadcast_command_handler},
        help::help_handler,
        refactor::{refactor_command_handler, understand_command_handler},
        settings::settings_command_handler,
        speech_recognition::speech_recognition_handler,
        start::start_handler,
        translate::translate_handler,
    },
    core::{config::Config, metrics::COMMANDS_COUNTER},
    errors::MyError,
    util::enums::Command,
};
use teloxide::{Bot, prelude::Message};
use tokio::task;

pub async fn command_handlers(bot: Bot, message: Message, cmd: Command) -> Result<(), MyError> {
    let config = Config::new().await;
    task::spawn(async move {
        let command_name = match &cmd {
            Command::Start(_) => "start",
            Command::Help => "help",
            Command::Translate(_) => "translate",
            Command::SpeechRecognition => "speech",
            Command::Settings => "settings",
            Command::Admin => "admin",
            Command::Broadcast => "broadcast",
            Command::Refactor => "refactor",
            Command::Understand => "understand",
        };
        COMMANDS_COUNTER.with_label_values(&[command_name]).inc();

        match cmd {
            Command::Start(arg) => start_handler(bot, message, &config, arg).await,
            Command::Help => help_handler(bot, message, &config, String::new()).await,
            Command::Translate(arg) => translate_handler(bot, &message, &config, arg).await,
            Command::SpeechRecognition => speech_recognition_handler(bot, message, &config).await,
            Command::Settings => settings_command_handler(bot, message, &config).await,

            Command::Admin => admin_command_handler(bot, message, &config).await,
            Command::Broadcast => broadcast_command_handler(bot, message, &config).await,

            Command::Refactor => refactor_command_handler(bot, message, &config).await,
            Command::Understand => understand_command_handler(bot, message, &config).await,
        }
    });
    Ok(())
}
