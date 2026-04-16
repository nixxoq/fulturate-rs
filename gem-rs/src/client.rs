//! Client module for interacting with the Gemini API.
//!
//! This module provides the main structures and implementations for creating and managing
//! sessions with the Gemini API, including support for sending messages, files, and blobs,
//! as well as streaming responses.

use super::types::Context;
use dotenv::dotenv;
use error::StreamBodyError;
use futures::Stream;
use reqwest::{Client as webClient, StatusCode};
use reqwest_streams::*;

use crate::api::{Models, DEFAULT_BASE_URL};
use crate::errors::GemError;
use crate::types::{
    BatchCreateInputConfig, BatchCreatePayload, BatchCreateRequest, BatchOperation, Blob,
    ErrorWrapper, FileData, GenerateContentResponse, ListOperationsResponse, Role, Settings,
};

pub type StreamResponseResult = Result<
    Box<dyn Stream<Item = Result<GenerateContentResponse, StreamBodyError>> + Unpin + Send>,
    GemError,
>;
pub type ResponseResult = Result<GenerateContentResponse, GemError>;

pub type StreamResponse = Box<
    dyn futures::Stream<
            Item = Result<GenerateContentResponse, reqwest_streams::error::StreamBodyError>,
        > + Unpin
        + Send,
>;

pub type Response = GenerateContentResponse;

/// Represents a session with the Gemini API.
pub struct GemSession {
    client: Client,
    context: Context,
}

/// Builder for creating a `GemSession` with custom configurations.
pub struct GemSessionBuilder(Config);

/// Internal configuration structure for `GemSessionBuilder`.
pub struct Config {
    timeout: Option<std::time::Duration>,
    connect_timeout: std::time::Duration,
    read_timeout: std::time::Duration,
    model: Models,
    context: Context,
    base_url: String,
}

impl GemSessionBuilder {
    /// Creates a new `GemSessionBuilder` with default settings.
    pub fn new() -> GemSessionBuilder {
        GemSessionBuilder(Config {
            timeout: None,
            connect_timeout: std::time::Duration::from_secs(30),
            read_timeout: std::time::Duration::from_secs(30),
            model: Models::default(),
            context: Context::new(),
            base_url: DEFAULT_BASE_URL.to_string(),
        })
    }

    /// Creates a default `GemSession` with the provided API key.
    pub fn default(api_key: String) -> GemSession {
        GemSession {
            client: Client::new(
                api_key,
                Models::default(),
                None,
                std::time::Duration::from_secs(30),
                std::time::Duration::from_secs(30),
                DEFAULT_BASE_URL.to_string(),
            ),
            context: Context::new(),
        }
    }

    /// Sets the timeout for API requests.
    /// By default, the timeout is none.
    /// When 'timeout' is set, 'read_timeout' is ignored according to the reqwest docs.
    /// Use for non-streaming requests. otherwise, the stream will be closed after the timeout
    /// even if the server is still responding.
    pub fn timeout(mut self, timeout: Option<std::time::Duration>) -> Self {
        self.0.timeout = timeout;
        self
    }

    /// Sets the Gemini model to use for the session.
    pub fn model(mut self, model: Models) -> Self {
        self.0.model = model;
        self
    }

    /// Sets a custom model to use for the session.
    pub fn custom_model(mut self, model: String) -> Self {
        self.0.model = Models::Custom(model);
        self
    }

    /// Sets the connection timeout for API requests.
    pub fn connect_timeout(mut self, connect_timeout: std::time::Duration) -> Self {
        self.0.connect_timeout = connect_timeout;
        self
    }

    /// Sets the read timeout for API requests.
    /// When 'timeout' is set, 'read_timeout' is ignored according to the reqwest docs.
    pub fn read_timeout(mut self, read_timeout: std::time::Duration) -> Self {
        self.0.read_timeout = read_timeout;
        self
    }

    /// Sets the initial context for the session.
    pub fn context(mut self, context: Context) -> Self {
        self.0.context = context;
        self
    }

    /// Sets a custom base URL for the Gemini API.
    pub fn base_url(mut self, url: &str) -> Self {
        self.0.base_url = url.trim_end_matches('/').to_string();
        self
    }

    /// Builds a `GemSession` with the configured settings and provided API key.
    pub fn build(self) -> GemSession {
        dotenv().ok();
        let api_key = std::env::var("GEMINI_API_KEY").expect("Failed to load Gemini API key");
        GemSession::build(api_key, self.0)
    }
}

/// Internal client for making API requests to Gemini.
pub struct Client {
    client: webClient,
    api_key: String,
    model: Models,
    base_url: String,
}

impl Client {
    /// Creates a new `Client` instance.
    pub fn new(
        api_key: String,
        model: Models,
        timeout: Option<std::time::Duration>,
        read_timeout: std::time::Duration,
        connect_timeout: std::time::Duration,
        base_url: String,
    ) -> Self {
        let mut client = webClient::builder()
            .read_timeout(read_timeout)
            .connect_timeout(connect_timeout);

        if let Some(timeout) = timeout {
            client = client.timeout(timeout);
        }

        Client {
            client: client.build().unwrap_or(webClient::new()),
            api_key,
            model,
            base_url,
        }
    }

    /// Sends a context to the Gemini API and returns the response.
    pub(crate) async fn send_context(
        &self,
        context: &Context,
        settings: &Settings,
    ) -> ResponseResult {
        let url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.base_url,
            self.model.to_string()
        );

        let context = context.build(settings);
        let response = match self
            .client
            .post(url)
            .query(&[("key", &self.api_key)])
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&context)
            .send()
            .await
        {
            Ok(response) => response,
            Err(e) => return Err(GemError::ConnectionError(e)),
        };

        let status_code = response.status();
        let response_text = match response.text().await {
            Ok(text) => text,
            Err(e) => return Err(GemError::ResponseError((e, status_code))),
        };

        let response = match status_code {
            StatusCode::OK => match serde_json::from_str::<GenerateContentResponse>(&response_text)
            {
                Ok(response) => response,
                Err(e) => {
                    return Err(GemError::ParsingError(e));
                }
            },
            _ => match serde_json::from_str::<ErrorWrapper>(&response_text) {
                Ok(error) => {
                    return Err(GemError::GeminiAPIError(error.error));
                }
                Err(e) => return Err(GemError::ParsingError(e)),
            },
        };

        if response.get_candidates().len() == 0 {
            return Err(GemError::EmptyApiResponse);
        }

        let mut blocked = true;
        for candidate in response.get_candidates() {
            if candidate.get_content().is_some()
            /*&& !candidate.is_blocked()*/
            {
                blocked = false;
                break;
            }
        }

        if blocked {
            if let Some(reason) = response.feedback() {
                return Err(GemError::FeedbackError(reason.to_string()));
            }
            return Err(GemError::AllCandidatesBlocked);
        }

        Ok(response)
    }

    /// Sends a context to the Gemini API and returns a stream of responses.
    pub(crate) async fn send_context_stream(
        &self,
        context: &Context,
        settings: &Settings,
    ) -> StreamResponseResult {
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent",
            self.base_url,
            self.model.to_string()
        );

        let response = self
            .client
            .post(url)
            .query(&[("key", &self.api_key)])
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&context.build(settings))
            .send()
            .await;

        match response {
            Ok(response) => {
                let status_code = response.status();
                match status_code {
                    StatusCode::OK => {
                        let json_stream = response.json_array_stream::<GenerateContentResponse>(
                            settings.get_stream_max_json_size() as usize,
                        );
                        Ok(Box::new(json_stream))
                    }
                    _ => {
                        return Err(GemError::StreamError(format!(
                            "Response error: {} (status code: {})",
                            response.text().await.unwrap(),
                            status_code
                        )));
                    }
                }
            }

            Err(e) => {
                return Err(GemError::ConnectionError(e));
            }
        }
    }

    pub(crate) async fn create_generate_content_batch_from_file(
        &self,
        model: &str,
        source_file_name: &str,
        _dest_file_name: &str,
        display_name: Option<&str>,
    ) -> Result<BatchOperation, GemError> {
        let model_name = if model.starts_with("models/") {
            model.to_string()
        } else {
            format!("models/{}", model)
        };

        let url = format!(
            "{}/v1beta/{}:batchGenerateContent",
            self.base_url, model_name
        );
        let payload = BatchCreateRequest {
            batch: BatchCreatePayload {
                input_config: BatchCreateInputConfig {
                    file_name: source_file_name.to_string(),
                },
                output_config: None,
                src: None,
                dest: None,
                display_name: display_name.map(ToOwned::to_owned),
            },
        };

        let response = self
            .client
            .post(url)
            .query(&[("key", &self.api_key)])
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(GemError::ConnectionError)?;

        let status_code = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| GemError::ResponseError((err, status_code)))?;

        if status_code.is_success() {
            return serde_json::from_str::<BatchOperation>(&text).map_err(GemError::ParsingError);
        }

        match serde_json::from_str::<ErrorWrapper>(&text) {
            Ok(error) => Err(GemError::GeminiAPIError(error.error)),
            Err(_) => Err(GemError::FileError(format!(
                "Batch create failed ({}): {}",
                status_code, text
            ))),
        }
    }

    pub(crate) async fn get_batch_operation(&self, name: &str) -> Result<BatchOperation, GemError> {
        let resource = if name.contains('/') {
            name.to_string()
        } else {
            format!("batches/{}", name)
        };
        let url = format!("{}/v1beta/{}", self.base_url, resource);

        let response = self
            .client
            .get(url)
            .query(&[("key", &self.api_key)])
            .send()
            .await
            .map_err(GemError::ConnectionError)?;

        let status_code = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| GemError::ResponseError((err, status_code)))?;

        if status_code.is_success() {
            if let Ok(operation) = serde_json::from_str::<BatchOperation>(&text) {
                return Ok(operation);
            }

            let value =
                serde_json::from_str::<serde_json::Value>(&text).map_err(GemError::ParsingError)?;

            let state = value
                .get("state")
                .and_then(|state| state.as_str())
                .unwrap_or_default();
            let done = matches!(
                state,
                "JOB_STATE_SUCCEEDED" | "JOB_STATE_FAILED" | "JOB_STATE_CANCELLED"
            );

            return Ok(BatchOperation {
                name: value
                    .get("name")
                    .and_then(|name| name.as_str())
                    .unwrap_or(&resource)
                    .to_string(),
                done,
                metadata: Some(value),
                response: None,
                error: None,
            });
        }

        match serde_json::from_str::<ErrorWrapper>(&text) {
            Ok(error) => Err(GemError::GeminiAPIError(error.error)),
            Err(_) => Err(GemError::FileError(format!(
                "Batch get failed ({}): {}",
                status_code, text
            ))),
        }
    }

    pub(crate) async fn list_batches(
        &self,
        page_size: Option<u32>,
        page_token: Option<&str>,
    ) -> Result<ListOperationsResponse, GemError> {
        async fn request(
            client: &webClient,
            base_url: &str,
            api_key: &str,
            path: &str,
            page_size: Option<u32>,
            page_token: Option<&str>,
        ) -> Result<reqwest::Response, reqwest::Error> {
            let url = format!("{}/v1beta/{}", base_url, path);
            let mut req = client.get(url).query(&[("key", api_key)]);

            if let Some(size) = page_size {
                req = req.query(&[("pageSize", size.to_string())]);
            }
            if let Some(token) = page_token {
                req = req.query(&[("pageToken", token)]);
            }

            req.send().await
        }

        let mut response = request(
            &self.client,
            &self.base_url,
            &self.api_key,
            "batches",
            page_size,
            page_token,
        )
        .await
        .map_err(GemError::ConnectionError)?;

        if response.status() == StatusCode::NOT_FOUND {
            response = request(
                &self.client,
                &self.base_url,
                &self.api_key,
                "operations",
                page_size,
                page_token,
            )
            .await
            .map_err(GemError::ConnectionError)?;
        }

        let status_code = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| GemError::ResponseError((err, status_code)))?;

        if !status_code.is_success() {
            if let Ok(error) = serde_json::from_str::<ErrorWrapper>(&text) {
                return Err(GemError::GeminiAPIError(error.error));
            }
            return Err(GemError::FileError(format!(
                "Batch list failed ({}): {}",
                status_code, text
            )));
        }

        if let Ok(parsed) = serde_json::from_str::<ListOperationsResponse>(&text) {
            return Ok(parsed);
        }

        let value =
            serde_json::from_str::<serde_json::Value>(&text).map_err(GemError::ParsingError)?;
        let mut operations = Vec::new();
        if let Some(batches) = value.get("batches").and_then(|v| v.as_array()) {
            for batch in batches {
                let state = batch
                    .get("state")
                    .and_then(|state| state.as_str())
                    .unwrap_or_default();
                let done = matches!(
                    state,
                    "JOB_STATE_SUCCEEDED" | "JOB_STATE_FAILED" | "JOB_STATE_CANCELLED"
                );
                operations.push(BatchOperation {
                    name: batch
                        .get("name")
                        .and_then(|name| name.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    done,
                    metadata: Some(batch.clone()),
                    response: None,
                    error: None,
                });
            }
        }

        Ok(ListOperationsResponse {
            operations,
            next_page_token: value
                .get("nextPageToken")
                .and_then(|token| token.as_str())
                .map(ToOwned::to_owned),
        })
    }

    pub(crate) async fn cancel_batch(&self, name: &str) -> Result<BatchOperation, GemError> {
        let resource = if name.contains('/') {
            name.to_string()
        } else {
            format!("batches/{}", name)
        };
        let url = format!("{}/v1beta/{}:cancel", self.base_url, resource);

        let response = self
            .client
            .post(url)
            .query(&[("key", &self.api_key)])
            .send()
            .await
            .map_err(GemError::ConnectionError)?;

        let status_code = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| GemError::ResponseError((err, status_code)))?;

        if status_code.is_success() {
            return serde_json::from_str::<BatchOperation>(&text).map_err(|_| {
                GemError::FileError(format!("Batch cancel response parse failed: {}", text))
            });
        }

        match serde_json::from_str::<ErrorWrapper>(&text) {
            Ok(error) => Err(GemError::GeminiAPIError(error.error)),
            Err(_) => Err(GemError::FileError(format!(
                "Batch cancel failed ({}): {}",
                status_code, text
            ))),
        }
    }
}

impl GemSession {
    /// Builds a new `GemSession` with the provided API key and configuration.
    pub(crate) fn build(api_key: String, config: Config) -> Self {
        GemSession {
            client: Client::new(
                api_key,
                config.model,
                config.timeout,
                config.read_timeout,
                config.connect_timeout,
                config.base_url,
            ),
            context: config.context,
        }
    }

    /// Creates a new `GemSession` with default settings and the provided API key.
    pub fn new(api_key: String) -> Self {
        GemSessionBuilder::default(api_key)
    }

    /// Returns a new `GemSessionBuilder` for creating a customized `GemSession`.
    pub fn Builder() -> GemSessionBuilder {
        GemSessionBuilder::new()
    }

    pub async fn create_generate_content_batch_from_file(
        &self,
        model: &str,
        source_file_name: &str,
        dest_file_name: &str,
        display_name: Option<&str>,
    ) -> Result<BatchOperation, GemError> {
        self.client
            .create_generate_content_batch_from_file(
                model,
                source_file_name,
                dest_file_name,
                display_name,
            )
            .await
    }

    pub async fn get_batch(&self, name: &str) -> Result<BatchOperation, GemError> {
        self.client.get_batch_operation(name).await
    }

    pub async fn list_batches(
        &self,
        page_size: Option<u32>,
        page_token: Option<&str>,
    ) -> Result<ListOperationsResponse, GemError> {
        self.client.list_batches(page_size, page_token).await
    }

    pub async fn cancel_batch(&self, name: &str) -> Result<BatchOperation, GemError> {
        self.client.cancel_batch(name).await
    }

    pub async fn wait_until_done(
        &self,
        operation_name: &str,
        poll_interval: std::time::Duration,
        timeout: std::time::Duration,
    ) -> Result<BatchOperation, GemError> {
        let started = std::time::Instant::now();

        loop {
            if started.elapsed() > timeout {
                return Err(GemError::FileError(format!(
                    "Batch operation timeout after {:?}",
                    timeout
                )));
            }

            let operation = self.get_batch(operation_name).await?;
            if operation.is_terminal() {
                return Ok(operation);
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Sends a message to the Gemini API and returns the response.
    pub async fn send_message(
        &mut self,
        message: &str,
        role: Role,
        settings: &Settings,
    ) -> ResponseResult {
        self.context.push_message(role, message.to_string());
        let response = self.send_context(settings).await?;
        if let Some(candidate) = response.get_candidates().first() {
            if let Some(content) = candidate.get_content() {
                self.context.push_message(
                    Role::Model,
                    match content.get_text() {
                        Some(text) => text.clone(),
                        None => return Err(GemError::EmptyApiResponse),
                    },
                );
            }
        }
        Ok(response)
    }

    /// Sends a file to the Gemini API and returns the response.
    pub async fn send_file(
        &mut self,
        file_data: FileData,
        role: Role,
        settings: &Settings,
    ) -> ResponseResult {
        self.context.push_file(role, file_data);

        let response = self.send_context(settings).await?;
        if let Some(candidate) = response.get_candidates().first() {
            if let Some(content) = candidate.get_content() {
                self.context.push_message(
                    Role::Model,
                    match content.get_text() {
                        Some(text) => text.clone(),
                        None => return Err(GemError::EmptyApiResponse),
                    },
                );
            }
        }
        Ok(response)
    }

    /// Sends a blob to the Gemini API and returns the response.
    pub async fn send_blob(
        &mut self,
        blob: Blob,
        role: Role,
        settings: &Settings,
    ) -> ResponseResult {
        self.context.push_blob(role, blob);
        let response = self.send_context(settings).await?;
        if let Some(candidate) = response.get_candidates().first() {
            if let Some(content) = candidate.get_content() {
                self.context.push_message(
                    Role::Model,
                    match content.get_text() {
                        Some(text) => text.clone(),
                        None => return Err(GemError::EmptyApiResponse),
                    },
                );
            }
        }
        Ok(response)
    }

    /// Sends a message with an attached file to the Gemini API and returns the response.
    pub async fn send_message_with_file(
        &mut self,
        message: &str,
        file_data: FileData,
        role: Role,
        settings: &Settings,
    ) -> ResponseResult {
        self.context
            .push_message_with_file(role, message, file_data);
        let response = self.send_context(settings).await?;
        if let Some(candidate) = response.get_candidates().first() {
            if let Some(content) = candidate.get_content() {
                self.context.push_message(
                    Role::Model,
                    match content.get_text() {
                        Some(text) => text.clone(),
                        None => return Err(GemError::EmptyApiResponse),
                    },
                );
            }
        }
        Ok(response)
    }

    /// Sends a message with an attached blob to the Gemini API and returns the response.
    pub async fn send_message_with_blob(
        &mut self,
        message: &str,
        blob: Blob,
        role: Role,
        settings: &Settings,
    ) -> ResponseResult {
        self.context.push_message_with_blob(role, message, blob);
        let response = self.send_context(settings).await?;
        if let Some(candidate) = response.get_candidates().first() {
            if let Some(content) = candidate.get_content() {
                self.context.push_message(
                    Role::Model,
                    match content.get_text() {
                        Some(text) => text.clone(),
                        None => return Err(GemError::EmptyApiResponse),
                    },
                );
            }
        }
        Ok(response)
    }

    /// Sends a message to the Gemini API and returns a stream of responses.
    pub async fn send_message_stream(
        &mut self,
        message: &str,
        role: Role,
        settings: &Settings,
    ) -> StreamResponseResult {
        self.context.push_message(role, message.to_string());
        Ok(Box::new(self.send_context_stream(settings).await?))
    }

    /// Sends a file to the Gemini API and returns a stream of responses.
    pub async fn send_file_stream(
        &mut self,
        file_data: FileData,
        role: Role,
        settings: &Settings,
    ) -> StreamResponseResult {
        self.context.push_file(role, file_data);
        Ok(Box::new(self.send_context_stream(settings).await?))
    }

    /// Sends a blob to the Gemini API and returns a stream of responses.
    pub async fn send_blob_stream(
        &mut self,
        blob: Blob,
        role: Role,
        settings: &Settings,
    ) -> StreamResponseResult {
        self.context.push_blob(role, blob);
        Ok(Box::new(self.send_context_stream(settings).await?))
    }

    /// Sends a message with an attached file to the Gemini API and returns a stream of responses.
    pub async fn send_message_with_file_stream(
        &mut self,
        message: &str,
        file_data: FileData,
        role: Role,
        settings: &Settings,
    ) -> StreamResponseResult {
        self.context
            .push_message_with_file(role, message, file_data);
        Ok(Box::new(self.send_context_stream(settings).await?))
    }

    /// Sends a message with an attached blob to the Gemini API and returns a stream of responses.
    pub async fn send_message_with_blob_stream(
        &mut self,
        message: &str,
        blob: Blob,
        role: Role,
        settings: &Settings,
    ) -> StreamResponseResult {
        self.context.push_message_with_blob(role, message, blob);
        Ok(Box::new(self.send_context_stream(settings).await?))
    }

    /// Internal method to send a context to the Gemini API.
    async fn send_context(&mut self, settings: &Settings) -> ResponseResult {
        self.client.send_context(&self.context, settings).await
    }

    /// Internal method to send a context to the Gemini API and return a stream of responses.
    async fn send_context_stream(&mut self, settings: &Settings) -> StreamResponseResult {
        self.client
            .send_context_stream(&self.context, settings)
            .await
    }
}

#[cfg(test)]
mod tests {

    use crate::types::HarmBlockThreshold;

    use super::*;

    #[tokio::test]
    async fn test_gem_session_send_context() {
        let mut session = GemSession::Builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(Some(std::time::Duration::from_secs(30)))
            .model(Models::Gemini25Flash)
            .context(Context::new())
            .build();

        let mut settings = Settings::new();
        settings.set_all_safety_settings(HarmBlockThreshold::BlockNone);
        settings.set_thinking_budget(4000);

        let response = session
            .send_message("Write me a poem about the moon", Role::User, &settings)
            .await;

        println!("Response: {:?}", response);
        assert!(response.is_ok())
    }
}
