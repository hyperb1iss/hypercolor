//! `hyper diagnose` -- system diagnostics and health checks.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;
use hypercolor_types::api::diagnose::DiagnoseRequest;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat};

/// The CLI's tolerant view of the daemon's diagnostics report.
///
/// The report body is deliberately daemon-local: its snapshot section
/// is internal telemetry with no shared contract, and a bug report
/// needs it verbatim. So this type names only the two sections the CLI
/// renders and carries the whole body alongside them, which keeps
/// --json and --report byte-for-byte what the daemon sent.
#[derive(Debug, Clone)]
struct DiagnoseReport {
    body: serde_json::Value,
    view: DiagnoseView,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct DiagnoseView {
    #[serde(default)]
    checks: Vec<DiagnoseCheckView>,
    #[serde(default)]
    summary: DiagnoseSummaryView,
}

#[derive(Debug, Clone, Deserialize)]
struct DiagnoseCheckView {
    category: String,
    name: String,
    status: String,
    detail: String,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct DiagnoseSummaryView {
    passed: u64,
    warnings: u64,
    failed: u64,
}

impl<'de> Deserialize<'de> for DiagnoseReport {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let body = serde_json::Value::deserialize(deserializer)?;
        let view = DiagnoseView::deserialize(&body).map_err(serde::de::Error::custom)?;
        Ok(Self { body, view })
    }
}

impl Serialize for DiagnoseReport {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.body.serialize(serializer)
    }
}

/// Run system diagnostics and health checks.
#[derive(Debug, Args)]
pub struct DiagnoseArgs {
    /// Run specific checks only (repeatable; includes `memory` and `macos_screen_parity`).
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
    let body = DiagnoseRequest {
        checks: (!args.check.is_empty()).then(|| args.check.clone()),
        system: Some(args.system),
    };

    let response: DiagnoseReport = client.post("/diagnose", &body).await?;

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
fn print_diagnostics_table(data: &DiagnoseReport, ctx: &OutputContext) {
    println!();
    ctx.info("Hypercolor Diagnostics");
    println!();

    let mut current_category = String::new();
    for check in &data.view.checks {
        // Print category header when it changes
        if check.category != current_category {
            let separator = "\u{2500}".repeat(50);
            println!("  \u{2500}\u{2500} {} {separator}", check.category);
            current_category.clone_from(&check.category);
        }

        let icon = ctx.painter.diagnose_icon(&check.status);
        let display_name = check.name.replace('_', " ");
        let detail = &check.detail;
        println!("  {icon} {display_name:<30} {detail}");
    }

    println!();
    print_summary(data);
}

/// Print the summary line.
fn print_summary(data: &DiagnoseReport) {
    let summary = data.view.summary;
    println!(
        "  Summary: {} passed, {} warnings, {} failed",
        summary.passed, summary.warnings, summary.failed
    );
}
