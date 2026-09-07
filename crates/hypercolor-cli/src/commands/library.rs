//! `hyper library` -- favorites, presets, and playlists.

use anyhow::Result;
use clap::{Args, Subcommand};
use hypercolor_types::api::library::{
    ActivatePlaylistResponse, ActivePlaylistStateResponse, AddFavoriteRequest, AddFavoriteResponse,
    DeactivatePlaylistResponse, DeleteFavoriteResponse, DeletePlaylistResponse,
    DeletePresetResponse, FavoriteListResponse, PlaylistItemRequest, PlaylistListResponse,
    PlaylistTargetRequest, PresetListResponse, SavePlaylistRequest, SavePresetRequest,
};
use hypercolor_types::api::scene::ApplyEffectRequest;
use hypercolor_types::library::{EffectPlaylist, EffectPreset};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, urlencoded};

/// Saved effect library operations.
#[derive(Debug, Args)]
pub struct LibraryArgs {
    #[command(subcommand)]
    pub command: LibraryCommand,
}

/// Top-level `library` subcommands.
#[derive(Debug, Subcommand)]
pub enum LibraryCommand {
    /// Favorite effects.
    Favorites(FavoritesArgs),
    /// Saved effect presets.
    Presets(PresetsArgs),
    /// Saved playlist sequences.
    Playlists(PlaylistsArgs),
}

/// Favorites command group.
#[derive(Debug, Args)]
pub struct FavoritesArgs {
    #[command(subcommand)]
    pub command: FavoritesCommand,
}

/// Favorites subcommands.
#[derive(Debug, Subcommand)]
pub enum FavoritesCommand {
    /// List favorited effects.
    List,
    /// Add or refresh a favorite effect.
    Add(FavoriteAddArgs),
    /// Remove a favorite effect.
    Remove(FavoriteRemoveArgs),
}

/// Arguments for `library favorites add`.
#[derive(Debug, Args)]
pub struct FavoriteAddArgs {
    /// Effect name or ID.
    pub effect: String,
}

/// Arguments for `library favorites remove`.
#[derive(Debug, Args)]
pub struct FavoriteRemoveArgs {
    /// Effect name or ID.
    pub effect: String,
}

/// Presets command group.
#[derive(Debug, Args)]
pub struct PresetsArgs {
    #[command(subcommand)]
    pub command: PresetsCommand,
}

/// Presets subcommands.
#[derive(Debug, Subcommand)]
pub enum PresetsCommand {
    /// Create a preset.
    Create(PresetCreateArgs),
    /// List saved presets.
    List,
    /// Show one preset.
    Info(PresetInfoArgs),
    /// Update an existing preset.
    Update(PresetUpdateArgs),
    /// Apply a preset.
    Apply(PresetApplyArgs),
    /// Delete a preset.
    Delete(PresetDeleteArgs),
}

/// Arguments for `library presets info`.
#[derive(Debug, Args)]
pub struct PresetInfoArgs {
    /// Preset ID or name.
    pub preset: String,
}

/// Arguments for `library presets apply`.
#[derive(Debug, Args)]
pub struct PresetApplyArgs {
    /// Preset ID or name.
    pub preset: String,
}

/// Arguments for `library presets delete`.
#[derive(Debug, Args)]
pub struct PresetDeleteArgs {
    /// Preset ID or name.
    pub preset: String,
    /// Skip confirmation prompt.
    #[arg(long)]
    pub yes: bool,
}

/// Arguments for `library presets update`.
#[derive(Debug, Args)]
pub struct PresetUpdateArgs {
    /// Preset ID or name.
    pub preset: String,
    /// JSON data with fields to update.
    #[arg(long)]
    pub data: String,
}

/// Arguments for `library presets create`.
#[derive(Debug, Args)]
pub struct PresetCreateArgs {
    /// Preset name.
    pub name: String,
    /// Effect ID or name.
    #[arg(long)]
    pub effect: String,
    /// Optional description.
    #[arg(long)]
    pub description: Option<String>,
    /// Repeatable control assignment (`key=value`).
    #[arg(long, short = 'c', value_parser = parse_key_value)]
    pub control: Vec<(String, String)>,
    /// Repeatable tag.
    #[arg(long, short = 't')]
    pub tag: Vec<String>,
}

/// Playlists command group.
#[derive(Debug, Args)]
pub struct PlaylistsArgs {
    #[command(subcommand)]
    pub command: PlaylistsCommand,
}

/// Playlists subcommands.
#[derive(Debug, Subcommand)]
pub enum PlaylistsCommand {
    /// Create a playlist.
    Create(PlaylistCreateArgs),
    /// List saved playlists.
    List,
    /// Show one playlist.
    Info(PlaylistInfoArgs),
    /// Update an existing playlist.
    Update(PlaylistUpdateArgs),
    /// Activate a playlist runtime.
    Activate(PlaylistActivateArgs),
    /// Show currently active playlist runtime.
    Active,
    /// Deactivate the active playlist runtime.
    Deactivate,
    /// Delete a playlist.
    Delete(PlaylistDeleteArgs),
}

/// Arguments for `library playlists info`.
#[derive(Debug, Args)]
pub struct PlaylistInfoArgs {
    /// Playlist ID or name.
    pub playlist: String,
}

/// Arguments for `library playlists activate`.
#[derive(Debug, Args)]
pub struct PlaylistActivateArgs {
    /// Playlist ID or name.
    pub playlist: String,
}

/// Arguments for `library playlists delete`.
#[derive(Debug, Args)]
pub struct PlaylistDeleteArgs {
    /// Playlist ID or name.
    pub playlist: String,
    /// Skip confirmation prompt.
    #[arg(long)]
    pub yes: bool,
}

/// Arguments for `library playlists update`.
#[derive(Debug, Args)]
pub struct PlaylistUpdateArgs {
    /// Playlist ID or name.
    pub playlist: String,
    /// JSON data with fields to update.
    #[arg(long)]
    pub data: String,
}

/// Parsed playlist item target kind.
#[derive(Debug, Clone, Copy)]
enum PlaylistItemKind {
    Effect,
    Preset,
}

/// Parsed CLI playlist item.
#[derive(Debug, Clone)]
pub struct PlaylistItemSpec {
    kind: PlaylistItemKind,
    target: String,
    duration_ms: Option<u64>,
    transition_ms: Option<u64>,
}

/// Arguments for `library playlists create`.
#[derive(Debug, Args)]
pub struct PlaylistCreateArgs {
    /// Playlist name.
    pub name: String,
    /// Optional description.
    #[arg(long)]
    pub description: Option<String>,
    /// Disable looping (default loop behavior is enabled).
    #[arg(long)]
    pub no_loop: bool,
    /// Repeatable item spec.
    ///
    /// Format:
    /// - `effect:<effect>`
    /// - `preset:<preset>`
    /// - optional `:duration_ms`
    /// - optional `:duration_ms:transition_ms`
    #[arg(long, short = 'i', value_parser = parse_playlist_item_spec)]
    pub item: Vec<PlaylistItemSpec>,
}

/// Execute `library` commands.
pub async fn execute(args: &LibraryArgs, client: &DaemonClient, ctx: &OutputContext) -> Result<()> {
    match &args.command {
        LibraryCommand::Favorites(group) => execute_favorites(group, client, ctx).await,
        LibraryCommand::Presets(group) => execute_presets(group, client, ctx).await,
        LibraryCommand::Playlists(group) => execute_playlists(group, client, ctx).await,
    }
}

async fn execute_favorites(
    args: &FavoritesArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    match &args.command {
        FavoritesCommand::List => {
            let response: FavoriteListResponse = client.get("/library/favorites").await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => {
                    for item in &response.items {
                        println!("{}", item.effect_name);
                    }
                }
                OutputFormat::Table => {
                    let headers = ["Effect", "Effect ID", "Added (ms)"];
                    let rows: Vec<Vec<String>> = response
                        .items
                        .iter()
                        .map(|item| {
                            vec![
                                ctx.painter.name(&item.effect_name),
                                ctx.painter.id(&item.effect_id),
                                ctx.painter.number(&item.added_at_ms.to_string()),
                            ]
                        })
                        .collect();
                    let row_count = rows.len();
                    ctx.print_table(&headers, &rows);
                    println!();
                    ctx.info(&format!(
                        "{} favorites",
                        ctx.painter.number(&row_count.to_string())
                    ));
                }
            }
        }
        FavoritesCommand::Add(add_args) => {
            let body = AddFavoriteRequest {
                effect: add_args.effect.clone(),
            };
            let response: AddFavoriteResponse = client.post("/library/favorites", &body).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    let effect = &response.favorite.effect_name;
                    if response.created {
                        ctx.success(&format!("Favorite added: {effect}"));
                    } else {
                        ctx.success(&format!("Favorite refreshed: {effect}"));
                    }
                }
            }
        }
        FavoritesCommand::Remove(remove_args) => {
            let path = format!("/library/favorites/{}", urlencoded(&remove_args.effect));
            let response: DeleteFavoriteResponse = client.delete(&path).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    ctx.success(&format!("Favorite removed: {}", remove_args.effect));
                }
            }
        }
    }

    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "preset command group includes all format render paths"
)]
async fn execute_presets(
    args: &PresetsArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    match &args.command {
        PresetsCommand::List => {
            let response: PresetListResponse = client.get("/library/presets").await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => {
                    for item in &response.items {
                        println!("{}", item.name);
                    }
                }
                OutputFormat::Table => {
                    let headers = ["Name", "ID", "Effect", "Tags", "Updated (ms)"];
                    let rows: Vec<Vec<String>> = response
                        .items
                        .iter()
                        .map(|item| {
                            let tags = item.tags.join(",");
                            vec![
                                ctx.painter.name(&item.name),
                                ctx.painter.id(&item.id.to_string()),
                                ctx.painter.muted(&item.effect_id.to_string()),
                                if tags.is_empty() {
                                    ctx.painter.muted("-")
                                } else {
                                    ctx.painter.muted(&tags)
                                },
                                ctx.painter.number(&item.updated_at_ms.to_string()),
                            ]
                        })
                        .collect();
                    let row_count = rows.len();
                    ctx.print_table(&headers, &rows);
                    println!();
                    ctx.info(&format!(
                        "{} presets",
                        ctx.painter.number(&row_count.to_string())
                    ));
                }
            }
        }
        PresetsCommand::Create(create_args) => {
            execute_create_preset(create_args, client, ctx).await?;
        }
        PresetsCommand::Info(info_args) => {
            let path = format!("/library/presets/{}", urlencoded(&info_args.preset));
            let response: EffectPreset = client.get(&path).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => {
                    println!("{}", response.name);
                }
                OutputFormat::Table => {
                    println!();
                    ctx.info(&format!("Preset: {}", response.name));
                    println!();
                    ctx.info(&format!("ID            {}", response.id));
                    ctx.info(&format!("Effect        {}", response.effect_id));
                    let tags = response.tags.join(", ");
                    if !tags.is_empty() {
                        ctx.info(&format!("Tags          {tags}"));
                    }
                    ctx.info(&format!("Updated (ms)  {}", response.updated_at_ms));
                    println!();
                }
            }
        }
        PresetsCommand::Apply(apply_args) => {
            let preset_path = format!("/library/presets/{}", urlencoded(&apply_args.preset));
            let preset: EffectPreset = client.get(&preset_path).await?;
            let effect_id = preset.effect_id.to_string();
            let path = format!(
                "/effects/{}/presets/{}/apply",
                urlencoded(&effect_id),
                urlencoded(&apply_args.preset)
            );
            // The preset-apply body has no named contract on purpose: the
            // daemon builds it with serde_json, which widens the f32
            // control values, so naming a shape would change the bytes.
            let response: serde_json::Value =
                client.post(&path, &ApplyEffectRequest::default()).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    ctx.success(&format!(
                        "Preset applied: {} -> effect {effect_id}",
                        apply_args.preset,
                    ));
                }
            }
        }
        PresetsCommand::Delete(delete_args) => {
            if !delete_args.yes {
                ctx.warning(&format!(
                    "Use --yes to confirm deletion of preset '{}'",
                    delete_args.preset
                ));
                return Ok(());
            }

            let path = format!("/library/presets/{}", urlencoded(&delete_args.preset));
            let response: DeletePresetResponse = client.delete(&path).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    ctx.success(&format!("Preset deleted: {}", delete_args.preset));
                }
            }
        }
        PresetsCommand::Update(update_args) => {
            let path = format!("/library/presets/{}", urlencoded(&update_args.preset));
            let body: serde_json::Value = serde_json::from_str(&update_args.data)
                .map_err(|e| anyhow::anyhow!("Invalid JSON: {e}"))?;
            let response: EffectPreset = client.put(&path, &body).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    ctx.success(&format!("Preset updated: {}", update_args.preset));
                }
            }
        }
    }

    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "playlist command group includes all format render paths"
)]
async fn execute_playlists(
    args: &PlaylistsArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    match &args.command {
        PlaylistsCommand::Create(create_args) => {
            execute_create_playlist(create_args, client, ctx).await?;
        }
        PlaylistsCommand::List => {
            let response: PlaylistListResponse = client.get("/library/playlists").await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => {
                    for item in &response.items {
                        println!("{}", item.name);
                    }
                }
                OutputFormat::Table => {
                    let headers = ["Name", "ID", "Items", "Loop", "Updated (ms)"];
                    let rows: Vec<Vec<String>> = response
                        .items
                        .iter()
                        .map(|item| {
                            vec![
                                ctx.painter.name(&item.name),
                                ctx.painter.id(&item.id.to_string()),
                                ctx.painter.number(&item.items.len().to_string()),
                                ctx.painter.yesno(item.loop_enabled),
                                ctx.painter.number(&item.updated_at_ms.to_string()),
                            ]
                        })
                        .collect();
                    let row_count = rows.len();
                    ctx.print_table(&headers, &rows);
                    println!();
                    ctx.info(&format!(
                        "{} playlists",
                        ctx.painter.number(&row_count.to_string())
                    ));
                }
            }
        }
        PlaylistsCommand::Info(info_args) => {
            let path = format!("/library/playlists/{}", urlencoded(&info_args.playlist));
            let response: EffectPlaylist = client.get(&path).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => println!("{}", response.name),
                OutputFormat::Table => {
                    println!();
                    ctx.info(&format!("Playlist: {}", response.name));
                    println!();
                    ctx.info(&format!("ID            {}", response.id));
                    ctx.info(&format!("Loop          {}", response.loop_enabled));
                    ctx.info(&format!("Items         {}", response.items.len()));
                    println!();
                }
            }
        }
        PlaylistsCommand::Activate(activate_args) => {
            let path = format!(
                "/library/playlists/{}/activate",
                urlencoded(&activate_args.playlist)
            );
            let response: ActivatePlaylistResponse =
                client.post(&path, &serde_json::json!({})).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    ctx.success(&format!("Playlist activated: {}", response.playlist.name));
                }
            }
        }
        PlaylistsCommand::Active => {
            let response: ActivePlaylistStateResponse =
                client.get("/library/playlists/active").await?;
            let playlist = &response.playlist;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => {
                    println!("{}", playlist.name);
                }
                OutputFormat::Table => {
                    println!();
                    ctx.info(&format!("Active Playlist: {}", playlist.name));
                    println!();
                    ctx.info(&format!("ID            {}", playlist.id));
                    ctx.info(&format!("Items         {}", playlist.item_count));
                    ctx.info(&format!("Started (ms)  {}", playlist.started_at_ms));
                    println!();
                }
            }
        }
        PlaylistsCommand::Deactivate => {
            let response: DeactivatePlaylistResponse = client
                .post("/library/playlists/deactivate", &serde_json::json!({}))
                .await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    ctx.success(&format!("Playlist deactivated: {}", response.playlist.name));
                }
            }
        }
        PlaylistsCommand::Delete(delete_args) => {
            if !delete_args.yes {
                ctx.warning(&format!(
                    "Use --yes to confirm deletion of playlist '{}'",
                    delete_args.playlist
                ));
                return Ok(());
            }

            let path = format!("/library/playlists/{}", urlencoded(&delete_args.playlist));
            let response: DeletePlaylistResponse = client.delete(&path).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    ctx.success(&format!("Playlist deleted: {}", delete_args.playlist));
                }
            }
        }
        PlaylistsCommand::Update(update_args) => {
            let path = format!("/library/playlists/{}", urlencoded(&update_args.playlist));
            let body: serde_json::Value = serde_json::from_str(&update_args.data)
                .map_err(|e| anyhow::anyhow!("Invalid JSON: {e}"))?;
            let response: EffectPlaylist = client.put(&path, &body).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    ctx.success(&format!("Playlist updated: {}", update_args.playlist));
                }
            }
        }
    }

    Ok(())
}

async fn execute_create_preset(
    args: &PresetCreateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let mut controls = serde_json::Map::new();
    for (key, value) in &args.control {
        controls.insert(key.clone(), parse_control_literal(value));
    }

    let body = SavePresetRequest {
        name: args.name.clone(),
        description: args.description.clone(),
        effect: args.effect.clone(),
        controls: Some(serde_json::Value::Object(controls)),
        tags: Some(args.tag.clone()),
    };
    let response: EffectPreset = client.post("/library/presets", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!("Preset created: {} ({})", args.name, response.id));
        }
    }

    Ok(())
}

async fn execute_create_playlist(
    args: &PlaylistCreateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let items: Vec<PlaylistItemRequest> = args
        .item
        .iter()
        .map(|item| {
            let target = match item.kind {
                PlaylistItemKind::Effect => PlaylistTargetRequest::Effect {
                    effect: item.target.clone(),
                },
                PlaylistItemKind::Preset => PlaylistTargetRequest::Preset {
                    preset_id: item.target.clone(),
                },
            };
            PlaylistItemRequest {
                target,
                duration_ms: item.duration_ms,
                transition_ms: item.transition_ms,
            }
        })
        .collect();

    let body = SavePlaylistRequest {
        name: args.name.clone(),
        description: args.description.clone(),
        loop_enabled: Some(!args.no_loop),
        items: Some(items),
    };
    let response: EffectPlaylist = client.post("/library/playlists", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            ctx.success(&format!(
                "Playlist created: {} ({})",
                args.name, response.id
            ));
        }
    }

    Ok(())
}

fn parse_key_value(s: &str) -> Result<(String, String), String> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=VALUE: no '=' found in '{s}'"))?;
    Ok((s[..pos].to_owned(), s[pos + 1..].to_owned()))
}

fn parse_control_literal(raw: &str) -> serde_json::Value {
    if raw.eq_ignore_ascii_case("true") {
        return serde_json::Value::Bool(true);
    }
    if raw.eq_ignore_ascii_case("false") {
        return serde_json::Value::Bool(false);
    }
    if let Ok(value) = raw.parse::<i64>() {
        return serde_json::json!(value);
    }
    if let Ok(value) = raw.parse::<f64>() {
        return serde_json::json!(value);
    }
    if raw.starts_with('[')
        && raw.ends_with(']')
        && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(raw)
    {
        return parsed;
    }
    serde_json::Value::String(raw.to_owned())
}

fn parse_playlist_item_spec(raw: &str) -> Result<PlaylistItemSpec, String> {
    let (kind, rest) = if let Some(rest) = raw.strip_prefix("effect:") {
        (PlaylistItemKind::Effect, rest)
    } else if let Some(rest) = raw.strip_prefix("preset:") {
        (PlaylistItemKind::Preset, rest)
    } else {
        return Err(format!(
            "invalid item '{raw}': expected prefix 'effect:' or 'preset:'"
        ));
    };

    if rest.trim().is_empty() {
        return Err(format!("invalid item '{raw}': missing target"));
    }

    let mut target = rest.to_owned();
    let mut duration_ms = None;
    let mut transition_ms = None;

    if let Some((head, tail)) = target.rsplit_once(':')
        && let Ok(parsed_tail) = tail.parse::<u64>()
    {
        target = head.to_owned();
        if let Some((head2, tail2)) = target.rsplit_once(':')
            && let Ok(parsed_tail2) = tail2.parse::<u64>()
        {
            duration_ms = Some(parsed_tail2);
            transition_ms = Some(parsed_tail);
            target = head2.to_owned();
        } else {
            duration_ms = Some(parsed_tail);
        }
    }

    if target.trim().is_empty() {
        return Err(format!("invalid item '{raw}': target must not be empty"));
    }

    Ok(PlaylistItemSpec {
        kind,
        target,
        duration_ms,
        transition_ms,
    })
}
