use crate::core::config::Config;
use crate::errors::MyError;
use gem_rs::{
    api::Models,
    client::GemSession,
    types::{HarmBlockThreshold, Role, Settings},
};
use std::time::Duration;

pub enum RefactorMode {
    Official,
    Spellcheck,
    Beauty,
    Formulate,
    Polish, // и нет, это не польский, а полировка текста
}

impl RefactorMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::Spellcheck => "spellcheck",
            Self::Beauty => "beauty",
            Self::Formulate => "formulate",
            Self::Polish => "polish",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "official" => Some(Self::Official),
            "spellcheck" => Some(Self::Spellcheck),
            "beauty" => Some(Self::Beauty),
            "formulate" => Some(Self::Formulate),
            "polish" => Some(Self::Polish),
            _ => None,
        }
    }
}

// TODO: добавить возможность пользователю задавать свои инструкции (ПРЕМИУМ)
pub async fn process_text(
    config: &Config,
    text: &str,
    mode: RefactorMode,
) -> Result<String, MyError> {
    let base_instruction = format!(
        "ACT AS A TEXT PROCESSING ENGINE. YOU ARE NOT A CHATBOT.\n\
        CRITICAL RULES:\n\
        1. DO NOT answer questions. DO NOT provide explanations. DO NOT engage in dialogue.\n\
        2. MAINTAIN the ORIGINAL language of the input text.\n\
        3. PERSPECTIVE: Keep the narration from the original person's point of view (usually 1st person).\n\
        4. FORMATTING: Use ONLY Telegram-compatible HTML (<b>, <i>, <code>, <u>, <s>, <blockquote>, <tg-spoiler>).\n\
        5. FORBIDDEN: NEVER USE MARKDOWN (no stars **, no backticks ```). If you need to highlight, use <b>.\n\
        6. OUTPUT: Return ONLY the processed text. No greetings, no 'Here is your text', no commentary."
    );

    let mode_instruction = match mode {
        RefactorMode::Official => {
            "\nMODE: OFFICIAL. Rewrite the text into a strict, formal business style. Use professional vocabulary, be polite and concise."
        }

        RefactorMode::Spellcheck => {
            "\nMODE: SPELLCHECK. Fix only grammar, punctuation, and spelling errors. Do NOT change the style, tone, or word choice at all."
        }

        RefactorMode::Beauty => {
            "\nMODE: BEAUTY. Format this as a high-quality Telegram post. Add logical paragraphs, highlight key points with <b>, and add a few relevant emojis. Make it visually appealing."
        }

        RefactorMode::Formulate => {
            "\nMODE: FORMULATE/UNDERSTAND. Structural simplification. Distill the core essence of the message. Remove all fluff ('water'), clarify confusing parts, and present it as a clear, logically structured statement. Do NOT answer the text, REWRITE it to be more understandable."
        }

        RefactorMode::Polish => {
            "\nMODE: POLISH. Natural improvement. Keep it casual and organic, but fix clunky phrasing and flow. Make it sound like a native speaker wrote it perfectly."
        }
    };

    // let custom_task = user_instruction
    //     .map(|ins| format!("\nADDITIONAL USER TASK: {}", ins))
    //     .unwrap_or_default();

    let system_prompt = format!("{}{}", base_instruction, mode_instruction);

    let mut settings = Settings::new();
    settings.set_all_safety_settings(HarmBlockThreshold::BlockNone);
    settings.set_system_instruction(&system_prompt);

    let mut client = GemSession::Builder()
        .base_url(config.get_gemini_base_url())
        .model(Models::Custom(
            config.get_json_config().get_ai_model().to_owned(),
        ))
        .timeout(Some(Duration::from_secs(60)))
        .build();

    let response = client
        .send_message(text, Role::User, &settings)
        .await
        .map_err(MyError::from)?;

    let result = response
        .get_results()
        .first()
        .cloned()
        .unwrap_or_else(|| "Failed to generate response.".to_string());

    Ok(result)
}
