//! `hyper library` -- favorites, presets, and playlists.

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::output::{OutputContext, OutputFormat, extract_str, urlencoded};

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
    /// Stop the active playlist runtime.
    Stop,
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
            let response = client.get("/library/favorites").await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => {
                    if let Some(items) = response.get("items").and_then(serde_json::Value::as_array)
                    {
                        for item in items {
                            println!("{}", extract_str(item, "effect_name"));
                        }
                    }
                }
                OutputFormat::Table => {
                    if let Some(items) = response.get("items").and_then(serde_json::Value::as_array)
                    {
                        let headers = ["Effect", "Effect ID", "Added (ms)"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                vec![
                                    extract_str(item, "effect_name"),
                                    extract_str(item, "effect_id"),
                                    item.get("added_at_ms")
                                        .and_then(serde_json::Value::as_u64)
                                        .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                                ]
                            })
                            .collect();
                        ctx.print_table(&headers, &rows);
                        println!();
                        ctx.info(&format!("{} favorites", rows.len()));
                    }
                }
            }
        }
        FavoritesCommand::Add(add_args) => {
            let body = serde_json::json!({ "effect": add_args.effect });
            let response = client.post("/library/favorites", &body).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    let created = response
                        .get("created")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    let effect = response
                        .get("favorite")
                        .and_then(|favorite| favorite.get("effect_name"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&add_args.effect);
                    if created {
                        ctx.success(&format!("Favorite added: {effect}"));
                    } else {
                        ctx.success(&format!("Favorite refreshed: {effect}"));
                    }
                }
            }
        }
        FavoritesCommand::Remove(remove_args) => {
            let path = format!("/library/favorites/{}", urlencoded(&remove_args.effect));
            let response = client.delete(&path).await?;
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
            let response = client.get("/library/presets").await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => {
                    if let Some(items) = response.get("items").and_then(serde_json::Value::as_array)
                    {
                        for item in items {
                            println!("{}", extract_str(item, "name"));
                        }
                    }
                }
                OutputFormat::Table => {
                    if let Some(items) = response.get("items").and_then(serde_json::Value::as_array)
                    {
                        let headers = ["Name", "ID", "Effect", "Tags", "Updated (ms)"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                let tags = item
                                    .get("tags")
                                    .and_then(serde_json::Value::as_array)
                                    .map_or_else(String::new, |values| {
                                        values
                                            .iter()
                                            .filter_map(serde_json::Value::as_str)
                                            .collect::<Vec<_>>()
                                            .join(",")
                                    });
                                vec![
                                    extract_str(item, "name"),
                                    extract_str(item, "id"),
                                    extract_str(item, "effect_id"),
                                    if tags.is_empty() {
                                        "-".to_owned()
                                    } else {
                                        tags
                                    },
                                    item.get("updated_at_ms")
                                        .and_then(serde_json::Value::as_u64)
                                        .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                                ]
                            })
                            .collect();
                        ctx.print_table(&headers, &rows);
                        println!();
                        ctx.info(&format!("{} presets", rows.len()));
                    }
                }
            }
        }
        PresetsCommand::Create(create_args) => {
            execute_create_preset(create_args, client, ctx).await?;
        }
        PresetsCommand::Info(info_args) => {
            let path = format!("/library/presets/{}", urlencoded(&info_args.preset));
            let response = client.get(&path).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => {
                    println!("{}", extract_str(&response, "name"));
                }
                OutputFormat::Table => {
                    println!();
                    ctx.info(&format!("Preset: {}", extract_str(&response, "name")));
                    println!();
                    ctx.info(&format!("ID            {}", extract_str(&response, "id")));
                    ctx.info(&format!(
                        "Effect        {}",
                        extract_str(&response, "effect_id")
                    ));
                    let tags = response
                        .get("tags")
                        .and_then(serde_json::Value::as_array)
                        .map_or_else(String::new, |values| {
                            values
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        });
                    if !tags.is_empty() {
                        ctx.info(&format!("Tags          {tags}"));
                    }
                    ctx.info(&format!(
                        "Updated (ms)  {}",
                        response
                            .get("updated_at_ms")
                            .and_then(serde_json::Value::as_u64)
                            .map_or_else(|| "-".to_owned(), |value| value.to_string())
                    ));
                    println!();
                }
            }
        }
        PresetsCommand::Apply(apply_args) => {
            let path = format!("/library/presets/{}/apply", urlencoded(&apply_args.preset));
            let response = client.post(&path, &serde_json::json!({})).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    let effect = response
                        .get("effect")
                        .and_then(|value| value.get("name"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?");
                    ctx.success(&format!(
                        "Preset applied: {} -> effect {effect}",
                        apply_args.preset
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
            let response = client.delete(&path).await?;
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
            let response = client.put(&path, &body).await?;
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
            let response = client.get("/library/playlists").await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => {
                    if let Some(items) = response.get("items").and_then(serde_json::Value::as_array)
                    {
                        for item in items {
                            println!("{}", extract_str(item, "name"));
                        }
                    }
                }
                OutputFormat::Table => {
                    if let Some(items) = response.get("items").and_then(serde_json::Value::as_array)
                    {
                        let headers = ["Name", "ID", "Items", "Loop", "Updated (ms)"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                let item_count = item
                                    .get("items")
                                    .and_then(serde_json::Value::as_array)
                                    .map_or(0, Vec::len);
                                vec![
                                    extract_str(item, "name"),
                                    extract_str(item, "id"),
                                    item_count.to_string(),
                                    item.get("loop_enabled")
                                        .and_then(serde_json::Value::as_bool)
                                        .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                                    item.get("updated_at_ms")
                                        .and_then(serde_json::Value::as_u64)
                                        .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                                ]
                            })
                            .collect();
                        ctx.print_table(&headers, &rows);
                        println!();
                        ctx.info(&format!("{} playlists", rows.len()));
                    }
                }
            }
        }
        PlaylistsCommand::Info(info_args) => {
            let path = format!("/library/playlists/{}", urlencoded(&info_args.playlist));
            let response = client.get(&path).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => println!("{}", extract_str(&response, "name")),
                OutputFormat::Table => {
                    println!();
                    ctx.info(&format!("Playlist: {}", extract_str(&response, "name")));
                    println!();
                    ctx.info(&format!("ID            {}", extract_str(&response, "id")));
                    ctx.info(&format!(
                        "Loop          {}",
                        response
                            .get("loop_enabled")
                            .and_then(serde_json::Value::as_bool)
                            .map_or_else(|| "?".to_owned(), |value| value.to_string())
                    ));
                    ctx.info(&format!(
                        "Items         {}",
                        response
                            .get("items")
                            .and_then(serde_json::Value::as_array)
                            .map_or(0, Vec::len)
                    ));
                    println!();
                }
            }
        }
        PlaylistsCommand::Activate(activate_args) => {
            let path = format!(
                "/library/playlists/{}/activate",
                urlencoded(&activate_args.playlist)
            );
            let response = client.post(&path, &serde_json::json!({})).await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    let name = response
                        .get("playlist")
                        .and_then(|playlist| playlist.get("name"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(&activate_args.playlist);
                    ctx.success(&format!("Playlist activated: {name}"));
                }
            }
        }
        PlaylistsCommand::Active => {
            let response = client.get("/library/playlists/active").await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain => {
                    let name = response
                        .get("playlist")
                        .and_then(|playlist| playlist.get("name"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?");
                    println!("{name}");
                }
                OutputFormat::Table => {
                    let playlist = response.get("playlist").unwrap_or(&serde_json::Value::Null);
                    println!();
                    ctx.info(&format!(
                        "Active Playlist: {}",
                        extract_str(playlist, "name")
                    ));
                    println!();
                    ctx.info(&format!("ID            {}", extract_str(playlist, "id")));
                    ctx.info(&format!(
                        "Items         {}",
                        playlist
                            .get("item_count")
                            .and_then(serde_json::Value::as_u64)
                            .map_or_else(|| "?".to_owned(), |value| value.to_string())
                    ));
                    ctx.info(&format!(
                        "Started (ms)  {}",
                        playlist
                            .get("started_at_ms")
                            .and_then(serde_json::Value::as_u64)
                            .map_or_else(|| "?".to_owned(), |value| value.to_string())
                    ));
                    println!();
                }
            }
        }
        PlaylistsCommand::Stop => {
            let response = client
                .post("/library/playlists/stop", &serde_json::json!({}))
                .await?;
            match ctx.format {
                OutputFormat::Json => ctx.print_json(&response)?,
                OutputFormat::Plain | OutputFormat::Table => {
                    let name = response
                        .get("playlist")
                        .and_then(|playlist| playlist.get("name"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?");
                    ctx.success(&format!("Playlist stopped: {name}"));
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
            let response = client.delete(&path).await?;
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
            let response = client.put(&path, &body).await?;
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

    let body = serde_json::json!({
        "name": args.name,
        "description": args.description,
        "effect": args.effect,
        "controls": controls,
        "tags": args.tag,
    });
    let response = client.post("/library/presets", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let id = extract_str(&response, "id");
            ctx.success(&format!("Preset created: {} ({id})", args.name));
        }
    }

    Ok(())
}

async fn execute_create_playlist(
    args: &PlaylistCreateArgs,
    client: &DaemonClient,
    ctx: &OutputContext,
) -> Result<()> {
    let items: Vec<serde_json::Value> = args
        .item
        .iter()
        .map(|item| {
            let target = match item.kind {
                PlaylistItemKind::Effect => serde_json::json!({
                    "type": "effect",
                    "effect": item.target,
                }),
                PlaylistItemKind::Preset => serde_json::json!({
                    "type": "preset",
                    "preset_id": item.target,
                }),
            };
            serde_json::json!({
                "target": target,
                "duration_ms": item.duration_ms,
                "transition_ms": item.transition_ms,
            })
        })
        .collect();

    let body = serde_json::json!({
        "name": args.name,
        "description": args.description,
        "loop_enabled": !args.no_loop,
        "items": items,
    });
    let response = client.post("/library/playlists", &body).await?;

    match ctx.format {
        OutputFormat::Json => ctx.print_json(&response)?,
        OutputFormat::Plain | OutputFormat::Table => {
            let id = extract_str(&response, "id");
            ctx.success(&format!("Playlist created: {} ({id})", args.name));
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
