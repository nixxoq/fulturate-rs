use fulturate::{
    bot::dispatcher::run,
    util::i18n::{check_and_update_locales, load_locales},
};
use log::{error, info};
use std::time::Duration;
use tokio::time::interval;

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();
    pretty_env_logger::init();

    load_locales();

    tokio::spawn(async move {
        let mut timer = interval(Duration::from_secs(4 * 60 * 60));
        timer.tick().await;

        loop {
            timer.tick().await;
            info!("[LOCALE] Checking GitHub for updates...");

            if check_and_update_locales().await {
                info!("[LOCALE] Updates detected. Reloading...");
                let _ = tokio::task::spawn_blocking(|| {
                    load_locales();
                })
                .await;
            } else {
                info!("[LOCALE] No updates found.");
            }
        }
    });

    info!("Bot starting...");

    match run().await {
        Ok(_) => info!("Bot stopped"),
        Err(e) => error!("Bot run failed: {}", e),
    }
}
