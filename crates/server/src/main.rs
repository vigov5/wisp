use std::net::SocketAddr;

use anyhow::Result;
use clap::{Parser, Subcommand};
use wisp_server::serve;

#[derive(Parser, Debug)]
#[command(
    name = "wisp-server",
    version,
    about = "Reference short-code server for wisp (Axum)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Serve {
        #[arg(long, default_value = "127.0.0.1:8787")]
        listen: SocketAddr,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Serve { listen } => serve(listen).await,
    }
}
