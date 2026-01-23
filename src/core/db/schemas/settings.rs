use crate::{
    bot::modules::{ModuleSettings, Owner},
    errors::MyError,
    impl_skeleton,
};
use mongodb::{
    bson,
    bson::{doc, oid::ObjectId},
};

use crate::bot::modules::registry::MOD_MANAGER;
use oximod::Model;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

fn default_true() -> bool {
    // true to true by true because true from true is true...
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Model)]
#[db("fulturate")]
#[collection("settings")]
pub struct Settings {
    #[serde(skip_serializing_if = "Option::is_none")]
    _id: Option<ObjectId>,

    #[index(unique, name = "owner")]
    pub owner_id: String,
    pub owner_type: String,

    #[serde(default)]
    pub language: String,

    #[serde(default = "default_true")]
    pub admin_only_mode: bool,

    #[serde(default)]
    pub modules: BTreeMap<String, Value>,
}

impl_skeleton!(Settings);

impl Settings {
    pub async fn create_with_defaults(owner: &Owner, language: String) -> Result<Self, MyError> {
        let mut modules_map = BTreeMap::<String, Value>::new();

        for module in MOD_MANAGER.get_all_modules() {
            if module.designed_for(&owner.r#type) {
                match module.factory_settings() {
                    Ok(settings_json) => {
                        modules_map.insert(module.key().to_string(), settings_json);
                    }
                    Err(e) => log::error!(
                        "Failed to get default settings for module '{}': {}",
                        module.key(),
                        e
                    ),
                }
            }
        }

        let new_doc = Settings::new()
            .owner_id(owner.id.clone())
            .owner_type(owner.r#type.clone())
            .language(language.clone())
            .admin_only_mode(true)
            .modules(modules_map);

        new_doc.save().await?;
        Ok(new_doc)
    }

    pub async fn get_module_settings<T: ModuleSettings>(
        owner: &Owner,
        module_key: &str,
    ) -> Result<T, MyError> {
        let settings_doc = Self::get_or_create(owner).await?;

        let module_settings = settings_doc.modules.get(module_key).map_or_else(
            || Ok(T::default()),
            |json_val| serde_json::from_value(json_val.clone()).map_err(MyError::from),
        )?;

        Ok(module_settings)
    }

    pub async fn update_module_settings<T: ModuleSettings>(
        owner: &Owner,
        module_key: &str,
        new_settings: T,
    ) -> Result<(), MyError> {
        let json_val = serde_json::to_value(new_settings)?;

        let result = Self::update_one(
            doc! { "owner_id": &owner.id, "owner_type": &owner.r#type },
            doc! { "$set": { format!("modules.{}", module_key): bson::to_bson(&json_val)? } },
        )
        .await?;

        if result.matched_count == 0 {
            let mut modules = BTreeMap::new();
            modules.insert(module_key.to_string(), json_val);

            let new_doc = Settings::new()
                .owner_id(owner.id.clone())
                .owner_type(owner.r#type.clone())
                .modules(modules);

            new_doc.save().await?;
        }

        Ok(())
    }

    pub async fn toggle_admin_mode(owner: &Owner) -> Result<bool, MyError> {
        let settings = Self::get_or_create(owner).await?;
        let new_state = !settings.admin_only_mode;

        Self::update_one(
            doc! { "owner_id": &owner.id, "owner_type": &owner.r#type },
            doc! { "$set": { "admin_only_mode": new_state } },
        )
        .await?;

        Ok(new_state)
    }

    pub(crate) async fn get_or_create(owner: &Owner) -> Result<Self, MyError> {
        let query = Self::query()
            .filter("owner_id", &owner.id)
            .filter("owner_type", &owner.r#type);

        if let Some(doc) = query.first().await? {
            return Ok(doc);
        }

        match Self::create_with_defaults(owner, "en".to_string()).await {
            // todo: fix duplicate key issue
            Ok(doc) => Ok(doc),
            Err(e) => {
                if e.to_string().contains("E11000") {
                    let query_retry = Self::query()
                        .filter("owner_id", &owner.id)
                        .filter("owner_type", &owner.r#type);

                    if let Some(doc) = query_retry.first().await? {
                        return Ok(doc);
                    }
                }
                Err(e)
            }
        }
    }
}
