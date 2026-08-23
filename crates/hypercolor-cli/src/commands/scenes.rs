//! `hyper scenes` -- reusable scene management.

use anyhow::Result;
use clap::{ArgAction, Args, Subcommand, ValueEnum};
use hypercolor_types::api::scene::SceneDocument;
use hypercolor_types::api::scenes::{
    ActivateSceneRequest, ActivateSceneResponse, CreateSceneRequest, DeleteSceneResponse,
    SceneListResponse, SceneSummary, SnapshotSceneRequest,
};
use hypercolor_types::scene::{SceneKind, SceneMutationMode};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, urlencoded};

/// Reusable scene management.
#[derive(Debug, Args)]
pub struct ScenesArgs {
    #[command(subcommand)]
    pub command: SceneCommand,
}

/// Scene subcommands.
#[derive(Debug, Subcommand)]
pub enum SceneCommand {
    /// List configured scenes.
    List,
    /// Show the currently active scene.
    Active,
    /// Create a new scene.
    Create(SceneCreateArgs),
    /// Snapshot the current runtime scene.
    Snapshot(SceneSnapshotArgs),
    /// Manually activate a scene.
    Activate(SceneActivateArgs),
    /// Return to the Default scene.
    Deactivate,
    /// Delete a scene.
    Delete(SceneDeleteArgs),
    /// Show detailed scene configuration.
    Info(SceneInfoArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SceneMutationModeArg {
    Live,
    Snapshot,
}

impl SceneMutationModeArg {
    const fn as_scene_mutation_mode(self) -> SceneMutationMode {
        match self {
            Self::Live => SceneMutationMode::Live,
            Self::Snapshot => SceneMutationMode::Snapshot,
        }
    }
}

/// Arguments for `scenes create`.
#[derive(Debug, Args)]
pub struct SceneCreateArgs {
    /// Scene name.
    pub name: String,

    #[arg(long)]
    pub description: Option<String>,

    /// Start enabled.
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    pub enabled: bool,

    /// Whether live runtime actions can rewrite this scene.
    #[arg(long, value_enum, default_value_t = SceneMutationModeArg::Live)]
    pub mutation_mode: SceneMutationModeArg,
}

/// Arguments for `scenes snapshot`.
#[derive(Debug, Args)]
pub struct SceneSnapshotArgs {
    /// Scene name.
    pub name: String,

    /// Scene description.
    #[arg(long)]
    pub description: Option<String>,
}

/// Arguments for `scenes activate`.
#[derive(Debug, Args)]
pub struct SceneActivateArgs {
    /// Scene name or ID.
    pub name: String,

    /// Override transition duration (ms).
    #[arg(long)]
    pub transition: Option<u32>,
}

/// Arguments for `scenes delete`.
#[derive(Debug, Args)]
pub struct SceneDeleteArgs {
    /// Scene name or ID.
    pub name: String,

    /// Skip confirmation.
    #[arg(long)]
    pub yes: bool,
}

/// Arguments for `scenes info`.
#[derive(Debug, Args)]
pub struct SceneInfoArgs {
    /// Scene name or ID.
    pub name: String,
}

/// Execute the `scenes` subcommand tree.
///
/// # Errors
///
/// Returns an error if the daemon is unreachable or the scene is not found.
pub async fn execute(args: &ScenesArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        SceneCommand::List => execute_list(client, ctx).await,
        SceneCommand::Active => execute_active(client, ctx).await,
        SceneCommand::Create(create_args) => execute_create(create_args, client, ctx).await,
        SceneCommand::Snapshot(snapshot_args) => execute_snapshot(snapshot_args, client, ctx).await,
        SceneCommand::Activate(activate_args) => execute_activate(activate_args, client, ctx).await,
        SceneCommand::Deactivate => execute_deactivate(client, ctx).await,
        SceneCommand::Delete(delete_args) => execute_delete(delete_args, client, ctx).await,
        SceneCommand::Info(info_args) => execute_info(info_args, client, ctx).await,
    }
}

async fn execute_snapshot(
    args: &SceneSnapshotArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let body = SnapshotSceneRequest {
        name: args.name.clone(),
        description: args.description.clone(),
    };
    let response: SceneSummary = client.post("/scenes/snapshot", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Scene snapshot saved: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_list(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response: SceneListResponse = client.get("/scenes").await?;
    let scenes = &response.items;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            for scene in scenes {
                println!("{}", scene.name);
            }
        }
        OutputFormat::Table => {
            let headers = ["ID", "Scene", "Mode", "Priority", "Enabled"];
            let rows: Vec<Vec<String>> = scenes
                .iter()
                .map(|s| {
                    vec![
                        ctx.painter.id(&s.id),
                        ctx.painter.name(&s.name),
                        mutation_mode_label(s.mutation_mode).to_owned(),
                        ctx.painter.number(&s.priority.to_string()),
                        ctx.painter.yesno(s.enabled),
                    ]
                })
                .collect();

            ctx.print_table(&headers, &rows);
            println!();
            ctx.info(&format!(
                "{} scenes",
                ctx.painter.number(&scenes.len().to_string())
            ));
        }
    }

    Ok(())
}

async fn execute_create(
    args: &SceneCreateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let body = CreateSceneRequest {
        name: args.name.clone(),
        description: args.description.clone(),
        enabled: Some(args.enabled),
        mutation_mode: Some(args.mutation_mode.as_scene_mutation_mode()),
    };

    let response: SceneSummary = client.post("/scenes", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Scene created: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_active(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response: SceneDocument = client.get("/scene").await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", response.name);
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&format!("Active Scene: {}", response.name));
            println!();
            ctx.info(&format!("ID             {}", response.id));
            ctx.info(&format!(
                "Kind           {}",
                scene_kind_label(response.kind)
            ));
            ctx.info(&format!("Zones          {}", response.zones.len()));
            println!();
        }
    }

    Ok(())
}

async fn execute_activate(
    args: &SceneActivateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/scenes/{}/activate", urlencoded(&args.name));
    let body = ActivateSceneRequest {
        transition_ms: args.transition.map(u64::from),
    };
    let response: ActivateSceneResponse = client.post(&path, &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Scene triggered: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_deactivate(client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    let response: SceneDocument = client
        .post("/scene/deactivate", &serde_json::json!({}))
        .await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success("Returned to Default scene");
        }
    }

    Ok(())
}

async fn execute_delete(
    args: &SceneDeleteArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    if !args.yes {
        ctx.warning(&format!(
            "Use --yes to confirm deletion of scene '{}'",
            args.name
        ));
        return Ok(());
    }

    let path = format!("/scenes/{}", urlencoded(&args.name));
    let response: DeleteSceneResponse = client.delete(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Scene deleted: {}", args.name));
        }
    }

    Ok(())
}

async fn execute_info(
    args: &SceneInfoArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let path = format!("/scenes/{}", urlencoded(&args.name));
    let response: SceneDocument = client.get(&path).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain => {
            println!("{}", response.name);
        }
        OutputFormat::Table => {
            println!();
            ctx.info(&format!("Scene: {}", response.name));
            println!();
            ctx.info(&format!("ID             {}", response.id));
            ctx.info(&format!(
                "Mutation Mode  {}",
                mutation_mode_label(response.mutation_mode)
            ));
            ctx.info(&format!("Priority       {}", response.priority.0));
            ctx.info(&format!(
                "Enabled        {}",
                if response.enabled { "yes" } else { "no" }
            ));
            println!();
        }
    }

    Ok(())
}

/// Wire spelling of a scene's mutation mode.
const fn mutation_mode_label(mode: SceneMutationMode) -> &'static str {
    match mode {
        SceneMutationMode::Live => "live",
        SceneMutationMode::Snapshot => "snapshot",
    }
}

/// Wire spelling of a scene's kind.
const fn scene_kind_label(kind: SceneKind) -> &'static str {
    match kind {
        SceneKind::Named => "named",
        SceneKind::Ephemeral => "ephemeral",
    }
}
