#[tokio::main]
async fn main() -> anyhow::Result<()> {
    hypercolor_cli::run().await
}
