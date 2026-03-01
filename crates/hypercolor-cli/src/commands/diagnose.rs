//! `hyper diagnose` -- system diagnostics and health checks.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

/// Run system diagnostics and health checks.
#[derive(Debug, Args)]
pub struct DiagnoseArgs {
    /// Run specific check(s) only (repeatable: daemon, devices, audio, render, config, permissions).
    #[arg(long)]
    pub check: Vec<String>,

    /// Generate a full diagnostic report file for bug reports.
    #[arg(long)]
    pub report: Option<PathBuf>,

    /// Include verbose system information (GPU, kernel, audio version, etc.).
    #[arg(long)]
    pub system: bool,
}

/// Execute the `diagnose` subcommand.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or diagnostics fail critically.
pub async fn execute(
    args: &DiagnoseArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let mut body = serde_json::json!({
        "system": args.system,
    });

    if !args.check.is_empty() {
        body["checks"] = serde_json::Value::Array(
            args.check
                .iter()
                .map(|c| serde_json::Value::String(c.clone()))
                .collect(),
        );
    }

    let response = client.post("/diagnose", &body).await?;

    // Write report file if requested
    if let Some(report_path) = &args.report {
        let report_content = serde_json::to_string_pretty(&response)?;
        std::fs::write(report_path, &report_content).map_err(|e| {
            anyhow::anyhow!("Failed to write report to {}: {e}", report_path.display())
        })?;
        ctx.success(&format!("Report written to {}", report_path.display()));
    }

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            print_summary(&response);
        }
        OutputFormat::Table => {
            print_diagnostics_table(&response, ctx);
        }
    }

    Ok(())
}

/// Print the diagnostic check results as a styled table.
fn print_diagnostics_table(data: &serde_json::Value, ctx: &OutputContext) {
    println!();
    ctx.info("Hypercolor Diagnostics");
    println!();

    if let Some(checks) = data.get("checks").and_then(serde_json::Value::as_array) {
        let mut current_category = String::new();

        for check in checks {
            let category = check
                .get("category")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let name = check
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let status = check
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let detail = check
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");

            // Print category header when it changes
            if category != current_category {
                let separator = "\u{2500}".repeat(50);
                println!("  \u{2500}\u{2500} {category} {separator}");
                current_category = category.to_string();
            }

            let icon = status_icon(status, ctx.color);
            let display_name = name.replace('_', " ");
            println!("  {icon} {display_name:<30} {detail}");
        }
    }

    println!();
    print_summary(data);
}

/// Print the summary line.
fn print_summary(data: &serde_json::Value) {
    if let Some(summary) = data.get("summary") {
        let passed = summary
            .get("passed")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let warnings = summary
            .get("warnings")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let failed = summary
            .get("failed")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);

        println!("  Summary: {passed} passed, {warnings} warnings, {failed} failed");
    }
}

/// Return a status icon based on check result.
fn status_icon(status: &str, color: bool) -> &'static str {
    if color {
        match status {
            "pass" => "\x1b[38;2;80;250;123m\u{2713}\x1b[0m",
            "warning" => "\x1b[38;2;241;250;140m!\x1b[0m",
            "fail" => "\x1b[38;2;255;99;99m\u{2717}\x1b[0m",
            _ => "?",
        }
    } else {
        match status {
            "pass" => "[OK]",
            "warning" => "[!!]",
            "fail" => "[FAIL]",
            _ => "[??]",
        }
    }
}
