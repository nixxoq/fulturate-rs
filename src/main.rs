use fulturate::{bot::dispatcher::run, util::i18n::load_locales};
use log::{error, info};
use std::time::Duration;
use tokio::time::interval;

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();
    pretty_env_logger::init();

    load_locales();

    tokio::spawn(async move {
        let mut timer = interval(Duration::from_secs(2 * 60 * 60));
        timer.tick().await;

        loop {
            timer.tick().await;
            info!("[LOCALE] reloading locales...");
            let _ = tokio::task::spawn_blocking(|| {
                load_locales();
            })
            .await;
        }
    });

    info!("Bot starting...");

    match run().await {
        Ok(_) => info!("Bot stopped"),
        Err(e) => error!("Bot run failed: {}", e),
    }
}
