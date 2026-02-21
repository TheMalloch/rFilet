mod routes;
mod state;
mod ws;

use axum::{Router, extract::DefaultBodyLimit};
use tower_http::cors::CorsLayer;

use tokio::net::TcpListener;
use tracing::info;

use axum::http::{Method, header};

use crate::state::AppState;

fn arg_value(flag: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
        if let Some((key, value)) = arg.split_once('=')
            && key == flag
        {
            return Some(value.to_string());
        }
    }
    None
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "filetransfer=info".into()),
        )
        .init();

    let state = AppState::new();
    let cleanup_state = state.clone();

    let cleanup_interval_seconds = std::env::var("CLEANUP_INTERVAL_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(60);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(cleanup_interval_seconds));
        interval.tick().await;
        loop {
            interval.tick().await;
            let deleted = cleanup_state.purge_expired();
            if deleted > 0 {
                info!("purged {deleted} expired files");
            }
        }
    });

    let cors = CorsLayer::new()
        .allow_origin(
            "https://comfrenzy.rasporar.org"
                .parse::<axum::http::HeaderValue>()
                .unwrap(),
        )
        .allow_methods([Method::GET, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE]);

    let app = Router::new()
        .route("/", axum::routing::get(routes::health))
        .route("/health", axum::routing::get(routes::health))
        .route("/d/{id}", axum::routing::get(routes::download_blob))
        .route("/ws/send", axum::routing::get(routes::ws_send))
        .layer(DefaultBodyLimit::max(1024 * 1024 * 1024))
        .layer(cors)
        .with_state(state);

    let port = arg_value("--port")
        .or_else(|| std::env::var("API_PORT").ok())
        .or_else(|| std::env::var("PORT").ok())
        .unwrap_or_else(|| "4020".to_string());
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await.unwrap();
    info!("filet api listening on port {port}, cleanup interval {cleanup_interval_seconds}s");
    axum::serve(listener, app).await.unwrap();
}
