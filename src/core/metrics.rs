use lazy_static::lazy_static;
use prometheus::{
    HistogramVec, IntCounter, IntCounterVec, register_histogram_vec, register_int_counter,
    register_int_counter_vec,
};

lazy_static! {
    pub static ref INCOMING_UPDATES: IntCounter = register_int_counter!(
        "fulturate_updates_total",
        "Total number of incoming updates"
    ).unwrap();

    pub static ref COMMANDS_COUNTER: IntCounterVec = register_int_counter_vec!(
        "fulturate_commands_total",
        "Total number of executed commands",
        &["command"]
    ).unwrap();

    pub static ref MODULE_USAGE: IntCounterVec = register_int_counter_vec!(
        "fulturate_module_usage",
        "Usage count per module",
        &["module", "action"] // module="cobalt", action="download_video"
    ).unwrap();

    pub static ref ERRORS_COUNTER: IntCounterVec = register_int_counter_vec!(
        "fulturate_errors_total",
        "Total errors encountered",
        &["type"]
    ).unwrap();

    pub static ref API_LATENCY: HistogramVec = register_histogram_vec!(
        "fulturate_api_duration_seconds",
        "External API response time",
        &["service"] // "gemini", "cobalt", "openai"
    ).unwrap();
}

pub async fn run_metrics_server() {
    use warp::Filter;

    let metrics_route = warp::path("metrics").map(|| {
        use prometheus::Encoder;
        let encoder = prometheus::TextEncoder::new();
        let mut buffer = vec![];
        let metric_families = prometheus::gather();
        encoder.encode(&metric_families, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    });

    log::info!("Metrics server starting on port 8082...");
    warp::serve(metrics_route).run(([0, 0, 0, 0], 8082)).await;
}
