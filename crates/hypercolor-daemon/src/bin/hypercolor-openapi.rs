use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let document =
        hypercolor_daemon::api::openapi::document_json_pretty().context("serialize OpenAPI")?;
    println!("{document}");
    Ok(())
}
