use teloxide::utils::command::BotCommands;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    #[command(description = "Start command")]
    Start(String),
    #[command(description = "Help command")]
    Help,
    #[command(description = "Speech recognition", alias = "sr")]
    SpeechRecognition,
    #[command(description = "Translate", alias = "tr")]
    Translate(String),
    #[command(description = "Bot settings")]
    Settings,

    #[command(description = "Admin Panel", hide)]
    Admin,
    #[command(description = "Broadcast message (reply only)", hide)]
    Broadcast,
}

pub struct AudioStruct {
    pub mime_type: String,
    pub file_id: String,
    pub file_unique_id: String,
}
