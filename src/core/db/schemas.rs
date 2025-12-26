pub mod group;
pub mod settings;
pub mod user;

use crate::core::{db::orm::QueryBuilder, services::currency::converter::CurrencyStruct};
use async_trait::async_trait;
use mongodb::{
    bson::{Document, doc},
    results::UpdateResult,
};
use oximod::_error::oximod_error::OxiModError;

pub trait IntoFilter {
    fn to_filter(self, key_field: &str) -> Document;
}

impl IntoFilter for String {
    fn to_filter(self, key_field: &str) -> Document {
        doc! { key_field: self }
    }
}

impl IntoFilter for &str {
    fn to_filter(self, key_field: &str) -> Document {
        doc! { key_field: self }
    }
}

impl IntoFilter for Document {
    fn to_filter(self, _key_field: &str) -> Document {
        self
    }
}

pub struct All;

impl IntoFilter for All {
    fn to_filter(self, _key_field: &str) -> Document {
        doc! {}
    }
}

impl IntoFilter for () {
    fn to_filter(self, _key_field: &str) -> Document {
        doc! {}
    }
}

#[async_trait]
pub trait BaseFunctions: Sized {
    async fn get<Q: IntoFilter + Send>(query: Q) -> Result<Option<Self>, OxiModError>;

    async fn list<Q: IntoFilter + Send>(query: Q) -> Result<Vec<Self>, OxiModError>;

    async fn create_with_id(id: String) -> Result<Self, OxiModError>;
}

#[async_trait]
pub trait CurrenciesFunctions: Sized {
    fn get_key(&self) -> &str;

    fn get_currencies(&self) -> &Vec<CurrencyStruct>;

    async fn add_currency(
        key: &str,
        currency: &CurrencyStruct,
    ) -> Result<UpdateResult, OxiModError>;

    async fn remove_currency(key: &str, currency_code: &str) -> Result<UpdateResult, OxiModError>;
}

pub trait OrmFunction: Sized {
    fn query() -> QueryBuilder<Self>;
}

#[macro_export]
macro_rules! impl_orm {
    ($struct_name:ident) => {
        use $crate::core::db::schemas::OrmFunction;

        impl OrmFunction for $struct_name {
            fn query() -> $crate::core::db::orm::QueryBuilder<Self> {
                $crate::core::db::orm::QueryBuilder::new()
            }
        }
    };
}

#[macro_export]
macro_rules! impl_base {
    ($struct_name:ident, $key_field:ident, $key_field_str:literal) => {
        use $crate::core::db::schemas::IntoFilter;

        #[async_trait::async_trait]
        impl BaseFunctions for $struct_name {
            async fn get<Q: IntoFilter + Send>(query: Q) -> Result<Option<Self>, OxiModError> {
                let filter = query.to_filter($key_field_str);
                <Self as OrmFunction>::query()
                    .filter_raw(filter)
                    .first()
                    .await
            }

            async fn list<Q: IntoFilter + Send>(query: Q) -> Result<Vec<Self>, OxiModError> {
                let filter = query.to_filter($key_field_str);
                <Self as OrmFunction>::query()
                    .filter_raw(filter)
                    .all()
                    .await
            }

            async fn create_with_id(id: String) -> Result<Self, OxiModError> {
                let mut entity = Self::new();
                entity = entity.$key_field(id.clone());
                entity.save().await?;

                Self::get(id).await?.ok_or_else(|| {
                    OxiModError::IndexError(format!(
                        "Failed to create {}",
                        stringify!($struct_name)
                    ))
                })
            }
        }

        impl $struct_name {
            pub async fn get_or_create_bool(id: &str) -> Result<bool, OxiModError> {
                if Self::get(id).await?.is_some() {
                    Ok(false)
                } else {
                    <Self as BaseFunctions>::create_with_id(id.to_string()).await?;
                    Ok(true)
                }
            }
        }
    };
}

#[macro_export]
macro_rules! impl_currencies {
    ($struct_name:ident, $key_field:ident, $key_field_str:literal) => {
        use $crate::core::db::schemas::CurrenciesFunctions;

        #[async_trait::async_trait]
        impl CurrenciesFunctions for $struct_name {
            fn get_key(&self) -> &str {
                &self.$key_field
            }
            fn get_currencies(
                &self,
            ) -> &Vec<$crate::core::services::currency::converter::CurrencyStruct> {
                &self.convertable_currencies
            }

            async fn add_currency(
                key: &str,
                currency: &CurrencyStruct,
            ) -> Result<mongodb::results::UpdateResult, OxiModError> {
                let currency_bson = mongodb::bson::to_bson(currency).unwrap();
                Self::update_one(
                    mongodb::bson::doc! { $key_field_str: key },
                    mongodb::bson::doc! { "$push": { "convertable_currencies": currency_bson } },
                )
                .await
            }

            async fn remove_currency(
                key: &str,
                code: &str,
            ) -> Result<mongodb::results::UpdateResult, OxiModError> {
                Self::update_one(
                    mongodb::bson::doc! { $key_field_str: key },
                    mongodb::bson::doc! { "$pull": { "convertable_currencies": { "code": code } } },
                )
                .await
            }
        }
    };
}

/// # Usage Patterns
///
/// 1. **ORM**:
///    ```rust
///    impl_skeleton!(Settings);
///    ```
///
/// 2. **ORM + Skelet**:
///    ```rust
///    impl_skeleton!(SimpleTable, id_field, "id_field");
///    ```
///
/// 3. **ORM + Skelet + Currencies**:
///    ```rust
///    impl_skeleton!(User, user_id, "user_id", +currencies);
///    ```
#[macro_export]
macro_rules! impl_skeleton {
    // ORM
    ($struct_name:ident) => {
        $crate::impl_orm!($struct_name);
    };

    // ORM + Skelet
    ($struct_name:ident, $key:ident, $key_str:literal) => {
        $crate::impl_orm!($struct_name);
        $crate::impl_base!($struct_name, $key, $key_str);
    };

    // ORM + Skelet + currency
    ($struct_name:ident, $key:ident, $key_str:literal, +currencies) => {
        $crate::impl_orm!($struct_name);
        $crate::impl_base!($struct_name, $key, $key_str);
        $crate::impl_currencies!($struct_name, $key, $key_str);
    };
}
