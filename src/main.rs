use std::process;

mod client;
mod config;
mod fileutil;

use client::Client;
use config::{load_config, Config};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const COMMIT: &str = "unknown";

// Simple random number generator to replace fastrand
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(12345);
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        // Linear congruential generator
        self.state = self.state.wrapping_mul(1103515245).wrapping_add(12345);
        self.state
    }

    fn usize(&mut self, max: usize) -> usize {
        (self.next_u64() % max as u64) as usize
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = pico_args::Arguments::from_env();

    // Handle help flag first
    if args.contains(["-h", "--help"]) {
        print_help();
        process::exit(0);
    }

    // Handle version flag
    if args.contains(["-V", "--version"]) {
        println!("Remdit Version: {}", VERSION);
        println!("Commit: {}", COMMIT);
        process::exit(0);
    }

    let verbose = args.contains(["-v", "--verbose"]);

    // Get the file path from free arguments
    let file_path: String = args
        .free_from_str()
        .map_err(|_| anyhow::anyhow!("Missing file argument"))?;

    // Check for unexpected arguments
    let remaining = args.finish();
    if !remaining.is_empty() {
        anyhow::bail!("Unexpected arguments: {:?}", remaining);
    }

    if verbose {
        println!("Debug mode enabled");
    }

    // Validate file
    if !fileutil::is_exist(&file_path) {
        eprintln!("File does not exist: {}", file_path);
        process::exit(1);
    }

    if fileutil::is_dir(&file_path) {
        eprintln!("{} is a directory, not a file", file_path);
        process::exit(1);
    }

    let abs_path = std::fs::canonicalize(&file_path)?;

    // Load config
    let config = load_config().await?;

    // Run the client
    run(config, abs_path, verbose).await?;

    Ok(())
}

fn print_help() {
    println!(
        "remdit {} - A collaborative text editor for remote files",
        VERSION
    );
    println!();
    println!("USAGE:");
    println!("    remdit [OPTIONS] <FILE>");
    println!();
    println!("ARGS:");
    println!("    <FILE>    The file to edit");
    println!();
    println!("OPTIONS:");
    println!("    -v, --verbose    Enable verbose output");
    println!("    -V, --version    Print version information");
    println!("    -h, --help       Print help information");
}

async fn run(config: Config, file_path: std::path::PathBuf, verbose: bool) -> anyhow::Result<()> {
    if config.servers.is_empty() {
        anyhow::bail!("No servers configured");
    }

    // Filter valid servers
    let valid_servers: Vec<_> = config
        .servers
        .into_iter()
        .filter(|server| server.is_valid())
        .collect();

    if valid_servers.is_empty() {
        anyhow::bail!("No valid servers found");
    }

    // Randomly select a server
    let selected_server = {
        let mut rng = SimpleRng::new();
        valid_servers[rng.usize(valid_servers.len())].clone()
    };

    if verbose {
        println!("Selected server: {}", selected_server.addr);
    }

    // Create and run client
    let mut client = Client::new(selected_server, file_path)?;

    client.create_session().await?;
    client.connect().await?;
    if verbose {
        println!("Connected to server: {}", client.server.addr);
    }

    let edit_url = client.get_edit_url();
    let file_name = client
        .file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    println!(
        "Edit URL for file {}: {}\nDO NOT SHARE TO STRANGERS!",
        file_name, edit_url
    );

    // Setup signal handling
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);
    let tx_clone = tx.clone();

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        let _ = tx_clone.send(()).await;
    });

    tokio::select! {
        result = client.handle_messages() => {
            match result {
                Ok(_) => {
                    if verbose {
                        println!("Session ended");
                    }
                }
                Err(e) => {
                    eprintln!("Error handling messages: {}", e);
                    client.close(1001, &e.to_string()).await?;
                    return Err(e);
                }
            }
        }
        _ = rx.recv() => {
            if verbose {
                println!("Received interrupt signal");
            }
        }
    }

    client.close(1000, "").await?;
    Ok(())
}
