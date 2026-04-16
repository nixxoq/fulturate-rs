use dotenv::dotenv;
use futures::stream::StreamExt;
use gem_rs::api::Models;
use gem_rs::client::GemSession;
use gem_rs::init_log;
use gem_rs::types::{Context, FileManager, HarmBlockThreshold, Role, Settings};

#[tokio::main]
async fn main() {
    init_log();
    test_custom_uri().await;
}

//TODO: Something with the API cause the cached files in cloud to change uri every time they are deleted
async fn test_clear_files() {
    let mut file_manager = FileManager::new();
    file_manager.fetch_list().await.unwrap();
    file_manager.clear_files().await;
}

// IMPORTANT:
// When 'timeout' is set, 'read_timeout' is ignored according to the reqwest docs.
// Use for non-streaming requests. otherwise, the stream will be closed after the timeout
// even if the server is still responding.

async fn test_stream() {
    let mut session = GemSession::Builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(30))
        .model(Models::Gemini25Flash)
        .context(Context::new())
        .build();

    let mut settings = Settings::new();
    settings.set_all_safety_settings(HarmBlockThreshold::BlockNone);
    settings.set_stream_max_json_size(16384);

    // While using a thinking model,
    // you may indicate to the user as "thinking" while
    // waiting for the first tokens, cause as far i know,
    // currently there's no way to get the thinking tokens
    // in the gemini APIs (if possible, PR!).
    println!("Thinking...");

    let stream_result = session
        .send_message_stream("Hello! tell me a long story", Role::User, &settings)
        .await;

    match stream_result {
        Ok(mut stream) => {
            while let Some(response) = stream.next().await {
                match response {
                    Ok(response) => {
                        println!(
                            "{}",
                            response.get_results().get(0).unwrap_or(&"".to_string())
                        );
                    }
                    Err(e) => {
                        println!("Error 1: {:?}", e);
                    }
                }
            }
        }
        Err(e) => {
            println!("Error 2: {:?}", e);
        }
    }
}

async fn test() {
    let mut session = GemSession::Builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(Some(std::time::Duration::from_secs(30)))
        .context(Context::new())
        .model(Models::Gemini25Pro)
        .build();

    let mut settings = Settings::new();
    settings.set_all_safety_settings(HarmBlockThreshold::BlockNone);

    // Set the thinking budget to 4000
    // This isn't necessary as the model will decide whether to think or not
    settings.set_thinking_budget(4000);

    let response = session
        .send_message("Hello! What is your name?", Role::User, &settings)
        .await;

    match response {
        Ok(response) => {
            println!("Response: {:#?}", response.get_results());
        }
        Err(e) => {
            println!("Error: {:?}", e);
        }
    }
}

async fn test_custom_uri() {
    dotenv().ok();
    let base_url = std::env::var("GEMINI_BASE_URL").expect("Failed to load Gemini base URL");

    let mut session = GemSession::Builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(30))
        .base_url(&base_url)
        .model(Models::Gemini25Flash)
        .context(Context::new())
        .build();

    let response = session
        .send_message("Hello! What is your name?", Role::User, &Settings::new())
        .await;

    match response {
        Ok(response) => {
            println!("Response: {:#?}", response.get_results());
        }
        Err(e) => {
            println!("Error: {:?}", e);
        }
    }
}
