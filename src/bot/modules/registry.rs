use super::{
    Module, cobalt::CobaltModule, currency::CurrencyModule, math::MathModule,
    speech_recognition::SpeechRecognitionModule, translate::TranslateModule,
    whisper::WhisperModule,
};
use once_cell::sync::Lazy;
use std::{collections::BTreeMap, sync::Arc};

pub struct ModuleManager {
    modules: BTreeMap<String, Arc<dyn Module>>,
}

impl ModuleManager {
    fn new() -> Self {
        macro_rules! register {
            ($($module:expr),* $(,)?) => {
                vec![ $( Arc::new($module) as Arc<dyn Module> ),* ]
            };
        }

        let modules = register![
            CobaltModule,
            CurrencyModule,
            WhisperModule,
            TranslateModule,
            MathModule,
            SpeechRecognitionModule,
        ];

        let modules = modules
            .into_iter()
            .map(|module| (module.key().to_string(), module))
            .collect();

        Self { modules }
    }

    pub fn get_module(&self, key: &str) -> Option<&Arc<dyn Module>> {
        self.modules.get(key)
    }

    pub fn get_all_modules(&self) -> Vec<&Arc<dyn Module>> {
        self.modules.values().collect()
    }

    pub fn get_designed_modules(&self, owner_type: &str) -> Vec<&Arc<dyn Module>> {
        self.modules
            .values()
            .filter(|module| module.designed_for(owner_type))
            .collect()
    }
}

pub static MOD_MANAGER: Lazy<ModuleManager> = Lazy::new(ModuleManager::new);
