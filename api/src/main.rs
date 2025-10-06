use axum::{Router, response::Json, routing::get};
use serde_json::{Value, json};
use std::net::SocketAddr;

async fn root_handler() -> Json<Value> {
    println!("GET / request received");
    Json(json!({ "status": "ok", "service": "fulturate_api" }))
}

#[tokio::main]
async fn main() {
    // Создаем роутер с одним маршрутом
    let app = Router::new().route("/", get(root_handler));

    // Адрес для запуска сервера. 0.0.0.0 нужен для работы внутри Docker.
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    println!("API server listening on {}", addr);

    // Запускаем сервер
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
