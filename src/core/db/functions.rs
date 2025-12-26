use crate::core::db::schemas::BaseFunctions;
use oximod::_error::oximod_error::OxiModError;

pub async fn get_or_create<T: BaseFunctions>(id: String) -> Result<T, OxiModError> {
    match T::get(id.clone()).await? {
        Some(entity) => Ok(entity),
        None => T::create_with_id(id).await,
    }
}
