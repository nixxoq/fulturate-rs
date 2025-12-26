use futures::stream::TryStreamExt;
use mongodb::bson::{Bson, Document, doc, from_document};
use oximod::{_error::oximod_error::OxiModError, Model};
use serde::Deserialize;
use std::marker::PhantomData;

pub struct QueryBuilder<T> {
    filter: Document,
    limit: Option<i64>,
    skip: Option<u64>,
    sort: Option<Document>,
    _marker: PhantomData<T>,
}

impl<T> QueryBuilder<T>
where
    T: Model + for<'de> Deserialize<'de> + Send + Sync + Unpin,
{
    pub fn new() -> Self {
        Self {
            filter: doc! {},
            limit: None,
            skip: None,
            sort: None,
            _marker: PhantomData,
        }
    }

    /// Appends an equality filter to the query.
    ///
    /// # Arguments
    /// * `field` - The database field name.
    /// * `value` - The value to match.
    pub fn filter<V: Into<Bson>>(mut self, field: &str, value: V) -> Self {
        self.filter.insert(field, value.into());
        self
    }

    /// Appends a filter using a specific MongoDB operator.
    ///
    /// # Arguments
    /// * `field` - The database field name.
    /// * `op` - The MongoDB operator (e.g., "$gt", "$lte").
    /// * `value` - The value for comparison.
    pub fn filter_op<V: Into<Bson>>(mut self, field: &str, op: &str, value: V) -> Self {
        self.filter.insert(field, doc! { op: value.into() });
        self
    }

    /// Appends an `$in` filter to match a value against a list of possibilities.
    pub fn filter_in<V: Into<Bson>>(mut self, field: &str, values: Vec<V>) -> Self {
        let bson_values: Vec<Bson> = values.into_iter().map(|v| v.into()).collect();
        self.filter.insert(field, doc! { "$in": bson_values });
        self
    }

    /// Merges a raw BSON `Document` into the current filter criteria.
    pub fn filter_raw(mut self, doc: Document) -> Self {
        self.filter.extend(doc);
        self
    }

    /// Specifies the maximum number of documents to return.
    pub fn limit(mut self, n: i64) -> Self {
        self.limit = Some(n);
        self
    }

    /// Specifies the number of documents to skip (offset).
    pub fn skip(mut self, n: u64) -> Self {
        self.skip = Some(n);
        self
    }

    /// Specifies the sort order for the query results.
    ///
    /// # Arguments
    /// * `field` - The field to sort by.
    /// * `direction` - `1` for ascending order, `-1` for descending order.
    pub fn sort(mut self, field: &str, direction: i32) -> Self {
        let mut sort_doc = self.sort.unwrap_or_else(|| doc! {});
        sort_doc.insert(field, direction);
        self.sort = Some(sort_doc);
        self
    }

    /// Executes the query and returns a vector of deserialized entities.
    ///
    /// This method retrieves the raw collection, applies the builder options (limit, skip, sort),
    /// and iterates over the cursor to deserialize documents into type `T`.
    pub async fn all(self) -> Result<Vec<T>, OxiModError> {
        let collection = T::get_collection()
            .map_err(|e| OxiModError::IndexError(format!("Failed to get collection: {}", e)))?;

        let mut find_action = collection.find(self.filter);

        if let Some(limit) = self.limit {
            find_action = find_action.limit(limit);
        }
        if let Some(skip) = self.skip {
            find_action = find_action.skip(skip);
        }
        if let Some(sort) = self.sort {
            find_action = find_action.sort(sort);
        }

        let mut cursor = find_action
            .await
            .map_err(|e| OxiModError::IndexError(format!("DB Find Error: {}", e)))?;

        let mut items = Vec::new();

        while let Some(doc) = cursor
            .try_next()
            .await
            .map_err(|e| OxiModError::IndexError(format!("DB Cursor Error: {}", e)))?
        {
            let item: T = from_document(doc)
                .map_err(|e| OxiModError::IndexError(format!("Deserialization Error: {}", e)))?;
            items.push(item);
        }

        Ok(items)
    }

    /// Executes the query and returns the first matching entity, if any.
    pub async fn first(mut self) -> Result<Option<T>, OxiModError> {
        self.limit = Some(1);
        let results = self.all().await?;
        Ok(results.into_iter().next())
    }

    /// Executes the query and returns the first entity.
    ///
    /// Returns `OxiModError::IndexError` if no document matches the criteria.
    pub async fn get(self) -> Result<T, OxiModError> {
        self.first()
            .await?
            .ok_or_else(|| OxiModError::IndexError("Document not found via ORM".to_string()))
    }

    /// Deletes all documents matching the current filter.
    pub async fn delete(self) -> Result<mongodb::results::DeleteResult, OxiModError> {
        let collection = T::get_collection()
            .map_err(|e| OxiModError::IndexError(format!("Failed to get collection: {}", e)))?;

        collection
            .delete_many(self.filter)
            .await
            .map_err(|e| OxiModError::IndexError(format!("DB Delete Error: {}", e)))
    }

    /// Checks if at least one document matches the current filter.
    pub async fn exists(self) -> Result<bool, OxiModError> {
        self.count().await.map(|c| c > 0)
    }

    /// Counts the total number of documents matching the current filter.
    pub async fn count(self) -> Result<u64, OxiModError> {
        let collection = T::get_collection()
            .map_err(|e| OxiModError::IndexError(format!("Failed to get collection: {}", e)))?;

        collection
            .count_documents(self.filter)
            .await
            .map_err(|e| OxiModError::IndexError(format!("DB Count Error: {}", e)))
    }
}
