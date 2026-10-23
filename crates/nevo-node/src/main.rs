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

use nevo_node::accounts::{Accounts, ACCOUNTS_FILE};
use nevo_node::{housekeeping, identity, router, AppState};
use nevo_search::{Config as SearchConfig, SearchIndex};
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
    let accounts = Accounts::open(args.data.join(ACCOUNTS_FILE)).map_err(|e| format!("accounts: {e}"))?;
    println!("NEVO node {} ", node.public());
    println!("data:   {}", args.data.display());
    let setup_code = accounts.setup_code();

    let store = Arc::new(store);
    housekeeping::spawn(store.clone());
    let search = Arc::new(
        SearchIndex::open(args.data.join("search"), SearchConfig::default())
            .map_err(|e| format!("search index: {e}"))?,
    );
    if search.num_docs() == 0 {
        // First start, or the index was deleted to rebuild it: index the library in the background.
        let (store, search) = (store.clone(), search.clone());
        tokio::task::spawn_blocking(move || match nevo_node::search::reindex_all(&store, &search) {
            Ok(n) => println!("search: indexed {n} items"),
            Err(_) => eprintln!("search: indexing the library failed"),
        });
    }
    let app = router(AppState::new(store, Arc::new(node), Arc::new(accounts)).with_search(search));
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .map_err(|e| format!("can't listen on {}: {e}", args.listen))?;
    println!("open:   http://{}/", args.listen);
    if let Some(code) = setup_code {
        // Only someone who can see this console can finish setup (D-052).
        println!();
        println!("This NEVO is not set up yet. Open it in a browser and enter this setup code:");
        println!();
        println!("        {code}");
        println!();
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}
