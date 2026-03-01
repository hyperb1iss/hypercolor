use clap::Parser;

/// Hypercolor CLI — control your RGB lighting from the terminal.
#[derive(Parser)]
#[command(name = "hyper", version, about)]
struct Cli {
    /// Output format
    #[arg(long, default_value = "table")]
    format: String,

    /// Daemon host
    #[arg(long, default_value = "localhost")]
    host: String,

    /// Daemon port
    #[arg(long, default_value = "9420")]
    port: u16,
}

fn main() {
    let _cli = Cli::parse();
}
