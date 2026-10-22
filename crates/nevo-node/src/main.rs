//! `nevo`: starts a NEVO node.
//!
//! ```text
//! nevo [--data <folder>] [--listen <address:port>]
//! ```
//!
//! Defaults: data in `./nevo-data`, listening on `0.0.0.0:8080` (every network interface, so
//! phones on the same Wi-Fi can connect).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use nevo_node::{housekeeping, identity, router, AppState};
use nevo_store::Store;

struct Args {
    data: PathBuf,
    listen: SocketAddr,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        data: PathBuf::from("nevo-data"),
        listen: "0.0.0.0:8080".parse().unwrap(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data" => args.data = it.next().ok_or("--data needs a folder")?.into(),
            "--listen" => {
                let value = it.next().ok_or("--listen needs an address such as 0.0.0.0:8080")?;
                args.listen = value.parse().map_err(|_| format!("not an address:port: {value}"))?;
            }
            "-h" | "--help" => {
                println!("nevo [--data <folder>] [--listen <address:port>]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    Ok(args)
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nevo: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    std::fs::create_dir_all(&args.data).map_err(|e| format!("can't create {}: {e}", args.data.display()))?;
    let node = identity::load_or_create(&args.data).map_err(|e| format!("node key: {e}"))?;
    let store = Store::open(args.data.join("library")).map_err(|e| format!("library: {e}"))?;
    println!("NEVO node {} ", node.public());
    println!("data:   {}", args.data.display());

    let store = Arc::new(store);
    housekeeping::spawn(store.clone());
    let app = router(AppState::new(store, Arc::new(node)));
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .map_err(|e| format!("can't listen on {}: {e}", args.listen))?;
    println!("open:   http://{}/", args.listen);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}
