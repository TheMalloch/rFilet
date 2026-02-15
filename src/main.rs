mod cli_routes;
mod cli_state;
mod cli_ws;
mod routes;
mod state;
mod static_assets;
mod ws;

use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use clap::{Parser, Subcommand};
use rand::RngCore;
use std::path::PathBuf;
use std::time::Duration;

use tokio::net::TcpListener;
use tracing::info;

use crate::cli_state::{CliState, SharedFile};
use crate::state::{AppState, TransferState};

const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Parser)]
#[command(name = "filetransfer", about = "Encrypted file transfer")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the relay web server (default)
    Web {
        /// Port to listen on
        #[arg(short, long, default_value = "4010")]
        port: u16,
    },
    /// Serve files directly from this machine
    Serve {
        /// Files to share
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Port to listen on
        #[arg(short, long, default_value = "4010")]
        port: u16,
        /// Public hostname or IP for download links
        #[arg(long)]
        host: String,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "filetransfer=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        None | Some(Commands::Web { .. }) => {
            let port = match &cli.command {
                Some(Commands::Web { port }) => *port,
                _ => std::env::var("PORT")
                    .ok()
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(4010),
            };
            run_web(port).await;
        }
        Some(Commands::Serve { files, port, host }) => {
            run_serve(files, port, host).await;
        }
    }
}

async fn run_web(port: u16) {
    let state = AppState::new();

    let cleanup_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(CLEANUP_INTERVAL).await;
            cleanup_state
                .transfers
                .retain(|_id, entry| !matches!(entry, TransferState::Done));
        }
    });

    let app = Router::new()
        .route("/", axum::routing::get(routes::sender_page))
        .route("/d/{id}", axum::routing::get(routes::receiver_page))
        .route(
            "/api/transfer/{id}",
            axum::routing::get(routes::transfer_info),
        )
        .route("/ws/send", axum::routing::get(routes::ws_send))
        .route("/ws/recv/{id}", axum::routing::get(routes::ws_recv))
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await.unwrap();
    info!("filet listening on http://localhost:{port}");
    axum::serve(listener, app).await.unwrap();
}

async fn run_serve(files: Vec<PathBuf>, port: u16, host: String) {
    // Validate files exist
    for path in &files {
        if !path.exists() {
            eprintln!("error: file not found: {}", path.display());
            std::process::exit(1);
        }
        if !path.is_file() {
            eprintln!("error: not a file: {}", path.display());
            std::process::exit(1);
        }
    }

    // Check port availability
    let addr = format!("0.0.0.0:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: cannot bind to port {port}: {e}");
            std::process::exit(1);
        }
    };

    let state = CliState::new();
    let mut rng = rand::thread_rng();

    println!();
    for path in &files {
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let meta = std::fs::metadata(path).unwrap();
        let size = meta.len();

        let mime_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();

        let mut enc_key = [0u8; 32];
        rng.fill_bytes(&mut enc_key);

        let token = nanoid::nanoid!(12);
        let key_b64 = URL_SAFE_NO_PAD.encode(&enc_key);

        let port_suffix = if port == 80 {
            String::new()
        } else {
            format!(":{port}")
        };
        let link = format!("http://{host}{port_suffix}/d/{token}#{key_b64}");

        println!("  {} ({}) ", filename, format_size(size));
        println!("  {link}");
        println!("  curl -OJ http://{host}{port_suffix}/dl/{token}");
        println!();

        state.files.insert(
            token,
            SharedFile {
                path: path.canonicalize().unwrap_or_else(|_| path.clone()),
                filename,
                size,
                mime_type,
                enc_key,
            },
        );
    }

    let app = Router::new()
        .route("/d/{token}", axum::routing::get(cli_routes::download_page))
        .route(
            "/api/file/{token}",
            axum::routing::get(cli_routes::file_metadata),
        )
        .route(
            "/dl/{token}",
            axum::routing::get(cli_routes::direct_download),
        )
        .route(
            "/ws/dl/{token}",
            axum::routing::get(cli_routes::ws_download),
        )
        .with_state(state);

    info!("serving {n} file(s) on http://{host}:{port}", n = files.len());
    info!("press Ctrl+C to stop");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    info!("shutting down");
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    if bytes < 1024 * 1024 {
        return format!("{:.1} KB", bytes as f64 / 1024.0);
    }
    if bytes < 1024 * 1024 * 1024 {
        return format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0));
    }
    format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}
