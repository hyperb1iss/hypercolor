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
    /// List saved presets.
    List,
    /// Show one preset.
    Info(PresetInfoArgs),
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

/// Playlists command group.
#[derive(Debug, Args)]
pub struct PlaylistsArgs {
    #[command(subcommand)]
    pub command: PlaylistsCommand,
}

/// Playlists subcommands.
#[derive(Debug, Subcommand)]
pub enum PlaylistsCommand {
    /// List saved playlists.
    List,
    /// Show one playlist.
    Info(PlaylistInfoArgs),
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
    }

    Ok(())
}
