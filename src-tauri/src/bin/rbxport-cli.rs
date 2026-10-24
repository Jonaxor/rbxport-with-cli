use std::env;
use std::path::{Path, PathBuf};
use std::process;

#[derive(Debug, Clone)]
enum NodeKind {
    Playlist,
    Folder,
}

#[derive(Debug, Clone)]
enum NodeAction {
    Add { name: String, parent: String },
    Delete { id: String },
}

#[derive(Debug, Clone)]
enum Command {
    Import { paths: Vec<PathBuf>, no_analysis: bool },
    Analyse { paths: Vec<PathBuf>, all: bool },
    Export { destination: Option<PathBuf>, playlists: Vec<String>, eject: bool },
    Reset { force: bool },
    Node { kind: NodeKind, action: NodeAction },
}

fn usage() -> String {
    concat!(
        "rbxport-cli\n",
        "\n",
        "Usage:\n",
        "  rbxport-cli import [--no-analysis] <path> [<path> ...]\n",
        "  rbxport-cli analyse [--all] [<path> ...]\n",
        "  rbxport-cli export [--usb <path>] [--playlist <name>]... [--eject]\n",
        "  rbxport-cli reset --force\n",
        "  rbxport-cli playlist add <name> [parent]\n",
        "  rbxport-cli playlist delete <id>\n",
        "  rbxport-cli folder add <name> [parent]\n",
        "  rbxport-cli folder delete <id>\n",
        "  rbxport-cli --help\n",
        "\n",
        "Notes:\n",
        "  - Uses the detected rekordbox library on this machine.\n",
        "  - Import uses the same backend writer as the desktop app.\n",
        "  - A direct parent directory becomes the playlist name, without creating a source folder wrapper.\n",
        "  - Analyse decodes the audio and runs the shared BPM/key analysis.\n",
        "  - Reset removes all live songs, playlists and folders after a --force confirmation.\n",
        "  - Playlist and folder actions match the GUI's write semantics.\n",
        "  - Export validates the destination before writing and can eject the USB with --eject.\n",
    )
    .to_owned()
}

fn parse_node_action(kind: NodeKind, args: &mut dyn Iterator<Item = String>) -> Result<Command, String> {
    let verb = args.next().ok_or_else(|| format!("{}\nError: '{kind:?}' requires an action: add or delete.", usage()))?;
    match verb.as_str() {
        "add" | "create" => {
            let name = args.next().ok_or_else(|| format!("{}\nError: {kind:?} add requires a name.", usage()))?;
            let parent = args.next().unwrap_or_else(|| rbl_db::write::ROOT.to_owned());
            Ok(Command::Node { kind, action: NodeAction::Add { name, parent } })
        }
        "delete" | "remove" => {
            let id = args.next().ok_or_else(|| format!("{}\nError: {kind:?} delete requires an id.", usage()))?;
            Ok(Command::Node { kind, action: NodeAction::Delete { id } })
        }
        _ => Err(format!("{}\nError: unknown action '{verb}' for {kind:?}.", usage())),
    }
}

fn parse_args_from<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let command = args.next().ok_or_else(|| usage())?;

    match command.as_str() {
        "help" | "--help" | "-h" => Err(usage()),
        "import" => {
            let mut paths = Vec::new();
            let mut no_analysis = false;
            for arg in args {
                if arg == "--no-analysis" || arg == "--no-analyse" || arg == "-n" {
                    no_analysis = true;
                } else {
                    paths.push(PathBuf::from(arg));
                }
            }
            if paths.is_empty() {
                return Err(format!("{}\nError: import requires at least one path.", usage()));
            }
            Ok(Command::Import { paths, no_analysis })
        }
        "analyse" | "analyze" => {
            let mut all = false;
            let mut paths = Vec::new();
            for arg in args {
                if arg == "--all" || arg == "-a" {
                    all = true;
                } else {
                    paths.push(PathBuf::from(arg));
                }
            }
            if all && !paths.is_empty() {
                return Err(format!("{}\nError: analyse --all cannot be combined with explicit paths.", usage()));
            }
            if !all && paths.is_empty() {
                return Err(format!("{}\nError: analyse requires at least one path or --all.", usage()));
            }
            Ok(Command::Analyse { paths, all })
        }
        "export" => {
            let mut destination = None;
            let mut playlists = Vec::new();
            let mut eject = false;
            let mut args = args.peekable();
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--usb" | "--destination" => {
                        let value = args.next().ok_or_else(|| format!("{}\nError: {arg} requires a path.", usage()))?;
                        destination = Some(PathBuf::from(value));
                    }
                    "--playlist" | "-p" => {
                        let value = args.next().ok_or_else(|| format!("{}\nError: {arg} requires a playlist name.", usage()))?;
                        playlists.push(value);
                    }
                    "--eject" | "--eject-after-export" | "-e" => {
                        eject = true;
                    }
                    "--all" => {
                        // Default is all playlists; this flag is accepted as an explicit form.
                    }
                    value if value.starts_with('-') => {
                        return Err(format!("{}\nError: unknown export flag '{value}'.", usage()));
                    }
                    value => {
                        if destination.is_some() {
                            return Err(format!("{}\nError: export accepts only one destination path.", usage()));
                        }
                        destination = Some(PathBuf::from(value));
                    }
                }
            }
            Ok(Command::Export { destination, playlists, eject })
        }
        "reset" => {
            let mut force = false;
            let mut extras = Vec::new();
            for arg in args {
                if arg == "--force" || arg == "-f" {
                    force = true;
                } else {
                    extras.push(arg);
                }
            }
            if !extras.is_empty() {
                return Err(format!("{}\nError: reset accepts only --force or -f.", usage()));
            }
            Ok(Command::Reset { force })
        }
        "playlist" => parse_node_action(NodeKind::Playlist, &mut args),
        "folder" => parse_node_action(NodeKind::Folder, &mut args),
        _ => Err(format!("{}\nError: unknown command '{command}'.", usage())),
    }
}

fn parse_args() -> Result<Command, String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    parse_args_from(args)
}

fn expand_import_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut budget = rbl_db::import::WalkBudget::new(16, 500_000);
    for path in paths {
        let path = Path::new(path);
        if !path.is_dir() {
            files.push(path.to_path_buf());
            continue;
        }
        if budget.exhausted() {
            break;
        }
        files.extend(rbl_db::import::audio_files_in(path, &mut budget));
    }
    files
}

fn detect_library() -> Result<rbl_db::LibraryLocation, String> {
    rbl_db::detect().map_err(|error| format!("No rekordbox library was detected: {error}"))
}

fn write_analysis_for_track(
    location: &rbl_db::LibraryLocation,
    writer: &mut rbl_db::write::Writer,
    track_id: &str,
    path: &Path,
) -> Result<(), String> {
    let relative = writer
        .analysis_data_path_for(track_id)
        .map_err(|error| format!("Could not resolve an analysis path for {track_id}: {error}"))?;

    let audio = rbl_audio::decode_mono(path, Some(1800.0))
        .map_err(|error| format!("Could not decode {}: {error}", path.display()))?;

    let options = rbl_analysis::AnalysisPreset::Rbxport.options();
    let analysis = rbl_analysis::analyse_with(&audio.samples, audio.sample_rate, options);
    let beats: Vec<rbl_anlz::Beat> = analysis
        .tempo
        .beats
        .iter()
        .map(|beat| rbl_anlz::Beat {
            beat_number: beat.beat_number,
            tempo_x100: beat.tempo_x100,
            time_ms: beat.time_ms,
        })
        .collect();
    let columns: Vec<rbl_anlz::BandColumn> = analysis
        .waveform
        .columns
        .iter()
        .map(|column| rbl_anlz::BandColumn {
            low: column.low,
            mid: column.mid,
            high: column.high,
            peak: column.peak,
        })
        .collect();

    let dat_path = rbl_anlz::resolve(&location.share_root, &relative);
    let parent = dat_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create analysis directory {}: {error}", parent.display()))?;

    let share_ppth = rbl_anlz::share_tree_ppth(&path.to_string_lossy());
    let files = rbl_anlz::author_with_overview(
        &share_ppth,
        &beats,
        &columns,
        analysis.waveform.overview.as_slice().try_into().ok(),
        rbl_anlz::Existing {
            dat: rbl_anlz::Anlz::read(&dat_path).ok().as_ref(),
            ext: rbl_anlz::Anlz::read(&rbl_anlz::sibling(&dat_path, "EXT")).ok().as_ref(),
            two_ex: rbl_anlz::Anlz::read(&rbl_anlz::sibling(&dat_path, "2EX")).ok().as_ref(),
        },
    );

    let to_u32 = |value: f64| value.round().clamp(0.0, f64::from(u32::MAX)) as u32;
    let bpm_x100 = to_u32(analysis.tempo.bpm * 100.0);
    let duration_sec = to_u32(audio.duration_secs());
    let detected_key = analysis.key.as_ref().map(|key| key.name.clone());
    let key = detected_key.as_deref();

    std::fs::write(&dat_path, &files.dat)
        .map_err(|error| format!("Could not write {}: {error}", dat_path.display()))?;
    std::fs::write(rbl_anlz::sibling(&dat_path, "EXT"), &files.ext)
        .map_err(|error| format!("Could not write {}: {error}", rbl_anlz::sibling(&dat_path, "EXT").display()))?;
    std::fs::write(rbl_anlz::sibling(&dat_path, "2EX"), &files.two_ex)
        .map_err(|error| format!("Could not write {}: {error}", rbl_anlz::sibling(&dat_path, "2EX").display()))?;

    if let Some(key) = key {
        writer
            .ensure_detected_key(key)
            .map_err(|error| format!("Could not create key record for {key}: {error}"))?;
    }

    writer
        .set_analysis(
            track_id,
            &rbl_db::write::AnalysisWrite {
                bpm_x100,
                key,
                analysis_path: &relative,
                length_sec: Some(duration_sec),
            },
        )
        .map_err(|error| format!("Could not register analysis for {track_id}: {error}"))?;

    println!(
        "analysed {track_id}: {} -> BPM {:.2}, key {}",
        path.display(),
        analysis.tempo.bpm,
        detected_key.as_deref().unwrap_or("unknown")
    );

    Ok(())
}

fn playlist_target_for_path(path: &Path) -> Option<(Option<String>, String)> {
    let parent = path.parent()?;
    let playlist_name = parent.file_name()?.to_string_lossy().to_string();
    Some((None, playlist_name))
}

fn ensure_imported_track_in_parent_playlist(
    writer: &mut rbl_db::write::Writer,
    track_id: &str,
    path: &Path,
) -> Result<(), String> {
    let Some((_, playlist_name)) = playlist_target_for_path(path) else {
        return Ok(());
    };

    let playlist_id = match writer
        .child_named(rbl_db::write::ROOT, &playlist_name)
        .map_err(|error| format!("Could not find playlist '{playlist_name}': {error}"))? {
        Some(id) => id,
        None => writer
            .create_playlist(&playlist_name, rbl_db::write::ROOT)
            .map_err(|error| format!("Could not create playlist '{playlist_name}': {error}"))?,
    };

    writer
        .add_tracks(&playlist_id, &[track_id.to_owned()])
        .map_err(|error| format!("Could not add track {track_id} to '{playlist_name}': {error}"))?;

    Ok(())
}

fn cmd_import(paths: &[PathBuf], no_analysis: bool) -> Result<(), String> {
    let files = expand_import_paths(paths);
    if files.is_empty() {
        return Err("No audio files were found in the supplied paths.".to_owned());
    }

    let location = detect_library()?;
    let backup_dir = std::env::temp_dir().join("rbxport-cli-backups");
    if !backup_dir.exists() {
        std::fs::create_dir_all(&backup_dir)
            .map_err(|error| format!("Could not create backup dir: {error}"))?;
    }

    let mut writer = rbl_db::write::Writer::open(location.clone(), &backup_dir)
        .map_err(|error| format!("Could not open the library for writing: {error}"))?;
    writer.disable_automatic_backups();

    let outcomes = writer
        .import_files(&files)
        .map_err(|error| format!("Import failed: {error}"))?;

    for (path, outcome) in files.iter().zip(outcomes) {
        match outcome {
            rbl_db::write::ImportOutcome::Added(id) => {
                println!("added {id}: {}", path.display());
                if let Err(error) = ensure_imported_track_in_parent_playlist(&mut writer, &id, path) {
                    eprintln!("playlist placement failed for {id} {}: {error}", path.display());
                }
                if !no_analysis {
                    if let Err(error) = write_analysis_for_track(&location, &mut writer, &id, path) {
                        eprintln!("analysis failed for {id} {}: {error}", path.display());
                    }
                } else {
                    println!("skipped analysis for {id}: {}", path.display());
                }
            }
            rbl_db::write::ImportOutcome::Existing(id) => {
                println!("already in library {id}: {}", path.display());
                if let Err(error) = ensure_imported_track_in_parent_playlist(&mut writer, &id, path) {
                    eprintln!("playlist placement failed for {id} {}: {error}", path.display());
                }
            }
            rbl_db::write::ImportOutcome::Refused(reason) => {
                eprintln!("skipped {}: {reason}", path.display());
            }
        }
    }

    Ok(())
}

fn cmd_analyse(paths: &[PathBuf], all: bool) -> Result<(), String> {
    let location = detect_library()?;
    let backup_dir = std::env::temp_dir().join("rbxport-cli-backups");
    if !backup_dir.exists() {
        std::fs::create_dir_all(&backup_dir)
            .map_err(|error| format!("Could not create backup dir: {error}"))?;
    }

    let mut writer = rbl_db::write::Writer::open(location.clone(), &backup_dir)
        .map_err(|error| format!("Could not open the library for writing: {error}"))?;
    writer.disable_automatic_backups();

    if all {
        let rows = {
            let mut all_tracks = writer
                .library()
                .connection()
                .prepare("SELECT ID, FolderPath FROM djmdContent WHERE rb_local_deleted = 0 ORDER BY ID")
                .map_err(|error| format!("Could not list library tracks: {error}"))?;
            let mapped_rows = all_tracks
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                .map_err(|error| format!("Could not read library tracks: {error}"))?;
            let rows = mapped_rows
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| format!("Could not read library track rows: {error}"))?;
            rows
        };
        let mut count = 0_usize;
        for (track_id, file_path) in rows {
            let path = PathBuf::from(file_path);
            if !path.exists() {
                eprintln!("skipping {track_id}: {} (missing file)", path.display());
                continue;
            }
            write_analysis_for_track(&location, &mut writer, &track_id, &path)?;
            count += 1;
        }
        println!("analysed {count} live tracks in the library");
        return Ok(());
    }

    for path in paths {
        let track_id = writer.track_id_at(path)
            .map_err(|error| format!("Could not resolve track in library for {}: {error}", path.display()))?
            .ok_or_else(|| format!("{} is not in the library; import it first.", path.display()))?;

        write_analysis_for_track(&location, &mut writer, &track_id, path)?;
    }

    Ok(())
}

fn cmd_reset(force: bool) -> Result<(), String> {
    if !force {
        return Err(format!("{}\nError: reset requires --force to confirm deletion of all songs, folders and playlists.", usage()));
    }

    let location = detect_library()?;
    let mut library = rbl_db::Library::open(location, rbl_db::OpenMode::ReadWrite)
        .map_err(|error| format!("Could not open the library for reset: {error}"))?;

    let stamp = rbl_core::time::now();
    let tx = library.connection_mut().transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("Could not start a reset transaction: {error}"))?;

    // The visible library is entirely derived from rows whose `rb_local_deleted = 0`.
    let live_songs = tx.query_row("SELECT COUNT(*) FROM djmdContent WHERE rb_local_deleted = 0", [], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("Could not count live songs before reset: {error}"))?;
    let live_playlists = tx.query_row("SELECT COUNT(*) FROM djmdPlaylist WHERE rb_local_deleted = 0", [], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("Could not count live playlists before reset: {error}"))?;

    tx.execute(
        "UPDATE djmdContent SET rb_local_deleted = 1, updated_at = ?1 WHERE rb_local_deleted = 0",
        rusqlite::params![stamp],
    ).map_err(|error| format!("Could not clear live songs: {error}"))?;
    tx.execute(
        "UPDATE djmdSongPlaylist SET rb_local_deleted = 1, updated_at = ?1 WHERE rb_local_deleted = 0",
        rusqlite::params![stamp],
    ).map_err(|error| format!("Could not clear song/playlists memberships: {error}"))?;
    tx.execute(
        "UPDATE djmdPlaylist SET rb_local_deleted = 1, rb_local_usn = COALESCE(rb_local_usn, 0) + 1, updated_at = ?1 WHERE rb_local_deleted = 0 AND ID <> 'root'",
        rusqlite::params![stamp],
    ).map_err(|error| format!("Could not clear playlists and folders: {error}"))?;
    tx.commit().map_err(|error| format!("Could not commit the reset: {error}"))?;

    let tracks = library.live_track_count().map_err(|error| format!("Could not confirm reset track count: {error}"))?;
    let playlists = library.live_playlist_count().map_err(|error| format!("Could not confirm reset playlist count: {error}"))?;
    if tracks != 0 || playlists != 0 {
        return Err(format!("Reset did not clear the library: {} tracks, {} playlists remain.", tracks, playlists));
    }

    println!("reset library: {} tracks, {} playlists were cleared", live_songs, live_playlists);
    Ok(())
}

fn cmd_node(kind: NodeKind, action: NodeAction) -> Result<(), String> {
    let location = detect_library()?;
    let backup_dir = std::env::temp_dir().join("rbxport-cli-backups");
    if !backup_dir.exists() {
        std::fs::create_dir_all(&backup_dir)
            .map_err(|error| format!("Could not create backup dir: {error}"))?;
    }

    let mut writer = rbl_db::write::Writer::open(location, &backup_dir)
        .map_err(|error| format!("Could not open the library for writing: {error}"))?;
    writer.disable_automatic_backups();

    match action {
        NodeAction::Add { name, parent } => {
            match kind {
                NodeKind::Playlist => {
                    let id = writer
                        .create_playlist(&name, &parent)
                        .map_err(|error| format!("Could not create playlist '{name}': {error}"))?;
                    println!("created playlist {id}: {name} (parent: {parent})");
                }
                NodeKind::Folder => {
                    let id = writer
                        .create_folder(&name, &parent)
                        .map_err(|error| format!("Could not create folder '{name}': {error}"))?;
                    println!("created folder {id}: {name} (parent: {parent})");
                }
            }
        }
        NodeAction::Delete { id } => {
            let changed = writer
                .delete_playlist(&id)
                .map_err(|error| format!("Could not delete {kind:?} {id}: {error}"))?;
            println!("deleted {kind:?} {id}; rows affected: {}", changed.rows);
        }
    }

    Ok(())
}

fn same_root_or_current_drive(path: &Path, location: &rbl_db::LibraryLocation) -> bool {
    let library_root = location
        .master_db
        .parent()
        .unwrap_or_else(|| Path::new("/"));
    let share_root = location
        .share_root
        .parent()
        .unwrap_or_else(|| Path::new("/"));
    rbl_core::durable::same_root(path, library_root).unwrap_or(false)
        || rbl_core::durable::same_root(path, share_root).unwrap_or(false)
}

fn choose_usb_drive(location: &rbl_db::LibraryLocation) -> Result<PathBuf, String> {
    let choices = rbl_devices::list()
        .into_iter()
        .filter(|device| !same_root_or_current_drive(&device.mount_point, location))
        .collect::<Vec<_>>();
    if choices.is_empty() {
        return Err("No external USB drive is available. Plug in a USB stick and try again.".to_owned());
    }

    let items = choices
        .iter()
        .map(|device| {
            let size_gb = device.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
            let size_label = if size_gb >= 10.0 {
                format!("{:.0} GB", size_gb)
            } else {
                format!("{:.1} GB", size_gb)
            };
            format!("{} ({}) [{}]", device.name, device.mount_point.display(), size_label)
        })
        .collect::<Vec<_>>();
    let index = dialoguer::Select::new()
        .with_prompt("Select a USB drive")
        .items(&items)
        .interact()
        .map_err(|error| format!("Could not open the USB picker: {error}"))?;

    Ok(choices[index].mount_point.clone())
}

fn add_export_playlist_index(playlists: &rbl_index::Playlists, ids: &mut Vec<u64>, seen: &mut std::collections::HashSet<u64>, index: usize) {
    let Some(id) = playlists.ids.get(index).copied() else {
        return;
    };
    if seen.insert(id) {
        ids.push(id);
    }
}

fn add_export_playlist_ancestors(playlists: &rbl_index::Playlists, ids: &mut Vec<u64>, seen: &mut std::collections::HashSet<u64>, mut index: usize) {
    loop {
        add_export_playlist_index(playlists, ids, seen, index);
        let parent = playlists.parent.get(index).copied().unwrap_or(rbl_index::NO_ID);
        if parent == rbl_index::NO_ID || parent as usize >= playlists.len() {
            break;
        }
        index = parent as usize;
    }
}

fn export_playlist_ids(playlists: &rbl_index::Playlists, names: &[String]) -> Result<Vec<u64>, String> {
    if names.is_empty() {
        return Ok((0..playlists.len()).filter_map(|index| playlists.ids.get(index).copied()).collect());
    }

    let mut ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut missing = Vec::new();

    for name in names {
        let Some(index) = (0..playlists.len()).find(|&index| playlists.name(index) == name.as_str()) else {
            missing.push(name.clone());
            continue;
        };

        add_export_playlist_ancestors(playlists, &mut ids, &mut seen, index);
        if playlists.is_folder(index) {
            let mut stack = vec![index];
            while let Some(folder) = stack.pop() {
                for child in 0..playlists.len() {
                    let parent = playlists.parent.get(child).copied().unwrap_or(rbl_index::NO_ID);
                    if parent == folder as u32 {
                        add_export_playlist_index(playlists, &mut ids, &mut seen, child);
                        stack.push(child);
                    }
                }
            }
        }
    }

    if !missing.is_empty() {
        return Err(format!("Playlist(s) not found: {}", missing.join(", ")));
    }

    ids.sort_unstable();
    Ok(ids)
}

fn build_export_tracks(
    library: &rbl_index::Library,
    share_root: &Path,
    selected_ids: &[u64],
) -> Result<(Vec<rbl_export::SourceTrack>, Vec<rbl_export::SourcePlaylist>), String> {
    let playlists = library.playlists();
    let mut tracks = Vec::new();
    let mut row_to_track = std::collections::HashMap::<u32, usize>::new();
    let mut source_playlists = Vec::new();

    for playlist_id in selected_ids {
        let Some(index) = playlists.index_of(*playlist_id) else {
            return Err(format!("Playlist {playlist_id} is not in the library."));
        };
        let name = playlists.name(index).to_owned();

        let parent_id = playlists.parent.get(index).copied().unwrap_or(rbl_index::NO_ID);
        let parent_playlist_id = if parent_id == rbl_index::NO_ID {
            0
        } else {
            playlists.ids.get(parent_id as usize).copied().unwrap_or(0)
        };

        if playlists.is_folder(index) {
            source_playlists.push(rbl_export::SourcePlaylist {
                id: *playlist_id,
                name,
                parent_id: parent_playlist_id as u64,
                folder: true,
                track_indices: Vec::new(),
                ..Default::default()
            });
            continue;
        }

        let rows = if playlists.is_smart(index) {
            library.source_rows(&rbl_index::TrackSource::SmartPlaylist(index))
        } else {
            library.source_rows(&rbl_index::TrackSource::Playlist(index))
        };

        let mut track_indices = Vec::new();
        for row in rows {
            let track_index = match row_to_track.entry(row) {
                std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let source_row = row as usize;
                    let mut analysis = Vec::new();
                    let relative = library.analysis_path.get(source_row).to_owned();
                    if !relative.is_empty() {
                        let dat = rbl_anlz::resolve(share_root, &relative);
                        for ext in ["DAT", "EXT", "2EX"] {
                            let candidate = rbl_anlz::sibling(&dat, ext);
                            if let Ok(bytes) = std::fs::read(&candidate) {
                                analysis.push((ext.to_owned(), bytes));
                            }
                        }
                    }
                    let track = rbl_export::SourceTrack {
                        id: library.ids[source_row],
                        source_path: PathBuf::from(library.folder_path.get(source_row)),
                        title: library.title.get(source_row).to_owned(),
                        artist: library.artist_name(row).to_owned(),
                        album: library.album_name(row).to_owned(),
                        genre: library.genre_name(row).to_owned(),
                        label: library.label_name(row).to_owned(),
                        key: library.key_name(row).to_owned(),
                        comment: library.comment.get(source_row).to_owned(),
                        date_added: library.date_added.get(source_row).to_owned(),
                        release_date: library.release_date.get(source_row).to_owned(),
                        bpm_x100: library.bpm_x100[source_row],
                        duration_sec: u16::try_from(library.length_sec[source_row]).unwrap_or(0),
                        rating: library.rating[source_row],
                        color_id: library.color[source_row],
                        bitrate: library.bitrate[source_row],
                        sample_rate: library.sample_rate[source_row],
                        file_size: library.file_size[source_row],
                        artwork: {
                            let artwork = library.artwork_path.get(source_row);
                            if artwork.is_empty() { None } else { Some(share_root.join(artwork)) }
                        },
                        analysis,
                        ..Default::default()
                    };
                    tracks.push(track);
                    let next_index = tracks.len() - 1;
                    entry.insert(next_index);
                    next_index
                }
            };
            track_indices.push(track_index);
        }

        source_playlists.push(rbl_export::SourcePlaylist {
            id: *playlist_id,
            name,
            parent_id: parent_playlist_id as u64,
            folder: false,
            track_indices,
            ..Default::default()
        });
    }

    Ok((tracks, source_playlists))
}

fn cmd_export(destination: &Option<PathBuf>, playlist_names: &[String], eject: bool) -> Result<(), String> {
    let location = detect_library()?;
    let target = match destination {
        Some(path) => path.clone(),
        None => choose_usb_drive(&location)?,
    };
    if same_root_or_current_drive(&target, &location) {
        return Err(format!("Refusing to write to the current library drive: {}", target.display()));
    }
    if !target.exists() {
        std::fs::create_dir_all(&target)
            .map_err(|error| format!("Could not create export destination: {error}"))?;
    }
    if !target.is_dir() {
        return Err(format!("Export destination is not a directory: {}", target.display()));
    }

    let db = rbl_db::Library::open(location.clone(), rbl_db::OpenMode::ReadOnly)
        .map_err(|error| format!("Could not open the library for export: {error}"))?;
    let (library, _) = rbl_index::load(&db)
        .map_err(|error| format!("Could not index the library for export: {error}"))?;

    let playlists = library.playlists();
    let selected_ids = export_playlist_ids(&playlists, playlist_names)?;

    if selected_ids.is_empty() {
        return Err("No playlists were selected for the USB sync.".to_owned());
    }

    let (tracks, playlists) = build_export_tracks(&library, &location.share_root, &selected_ids)?;
    let report = rbl_export::export(&target, &tracks, &playlists)
        .map_err(|error| format!("USB sync failed: {error}"))?;

    println!("Synced {} tracks and {} playlists to {}", report.tracks, report.playlists, target.display());
    if !report.skipped.is_empty() {
        println!("Skipped {} tracks that were missing or unreadable.", report.skipped.len());
    }
    if eject {
        rbl_devices::eject::eject(&target)
            .map_err(|error| format!("Export succeeded, but ejecting {} failed: {error}", target.display()))?;
        println!("ejected {}", target.display());
    }

    Ok(())
}

fn main() {
    let command = match parse_args() {
        Ok(command) => command,
        Err(help) => {
            eprintln!("{help}");
            process::exit(1);
        }
    };

    let result = match command {
        Command::Import { paths, no_analysis } => cmd_import(&paths, no_analysis),
        Command::Analyse { paths, all } => cmd_analyse(&paths, all),
        Command::Export { destination, playlists, eject } => cmd_export(&destination, &playlists, eject),
        Command::Reset { force } => cmd_reset(force),
        Command::Node { kind, action } => cmd_node(kind, action),
    };

    if let Err(error) = result {
        eprintln!("Error: {error}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{expand_import_paths, parse_node_action, usage};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn usage_lists_commands() {
        let text = usage();
        assert!(text.contains("import"));
        assert!(text.contains("analyse"));
        assert!(text.contains("export"));
        assert!(text.contains("playlist"));
        assert!(text.contains("folder"));
    }

    #[test]
    fn parse_import_no_analysis_flag() {
        let parsed = super::parse_args_from(vec!["import".to_owned(), "--no-analysis".to_owned(), "/tmp/test.mp3".to_owned()]).unwrap();
        assert!(matches!(parsed, super::Command::Import { paths, no_analysis: true } if paths.len() == 1 && paths[0] == PathBuf::from("/tmp/test.mp3")));
    }

    #[test]
    fn parse_playlist_create_command() {
        let mut args = vec!["create", "Chilled", "root"].into_iter().map(str::to_owned);
        let parsed = parse_node_action(super::NodeKind::Playlist, &mut args).unwrap();
        match parsed {
            super::Command::Node { kind, action } => {
                assert!(matches!(kind, super::NodeKind::Playlist));
                assert!(matches!(action, super::NodeAction::Add { name, parent } if name == "Chilled" && parent == "root"));
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn parse_export_flags() {
        let parsed = super::parse_args_from(vec![
            "export".to_owned(),
            "--usb".to_owned(),
            "/media/usb".to_owned(),
            "--playlist".to_owned(),
            "Techno".to_owned(),
            "--playlist".to_owned(),
            "House".to_owned(),
            "--eject".to_owned(),
        ]).unwrap();
        assert!(matches!(parsed, super::Command::Export { destination: Some(path), playlists, eject: true } if path == PathBuf::from("/media/usb") && playlists == vec!["Techno".to_owned(), "House".to_owned()]));
    }

    #[test]
    fn parse_analyse_all_flag() {
        let parsed = super::parse_args_from(vec!["analyse".to_owned(), "--all".to_owned()]).unwrap();
        assert!(matches!(parsed, super::Command::Analyse { paths, all: true } if paths.is_empty()));
    }

    #[test]
    fn parse_reset_command_requires_force() {
        let parsed = super::parse_args_from(vec!["reset".to_owned()]).unwrap();
        assert!(matches!(parsed, super::Command::Reset { force: false }));

        let parsed = super::parse_args_from(vec!["reset".to_owned(), "--force".to_owned()]).unwrap();
        assert!(matches!(parsed, super::Command::Reset { force: true }));
    }

    #[test]
    fn playlist_target_for_a_nested_directory_is_parent_playlist_under_parent_folder() {
        let path = std::path::Path::new("/Music/Techno/Dub/Song.mp3");
        assert_eq!(super::playlist_target_for_path(path), Some((None, "Dub".to_owned())));
        assert_eq!(super::playlist_target_for_path(std::path::Path::new("/Music/Song.mp3")), Some((None, "Music".to_owned())));
    }

    #[test]
    fn export_playlist_ids_include_parent_folders_for_nested_selections() {
        let mut names = rbl_index::strings::StrColumn::default();
        names.push("Music");
        names.push("Dub");
        let playlists = rbl_index::Playlists {
            ids: vec![10, 20],
            names,
            parent: vec![rbl_index::NO_ID, 0],
            seq: vec![0, 0],
            attribute: vec![rbl_index::ATTRIBUTE_FOLDER, 0],
            smart: rbl_index::strings::StrColumn::default(),
            members: vec![Vec::new(), Vec::new()],
        }; 

        let selected = super::export_playlist_ids(&playlists, &["Dub".to_owned()]).unwrap();
        assert_eq!(selected, vec![10, 20]);

        let all = super::export_playlist_ids(&playlists, &[]).unwrap();
        assert_eq!(all, vec![10, 20]);
    }

    #[test]
    fn expand_import_paths_walks_nested_directory_trees() {
        let base = std::env::temp_dir().join(format!(
            "rbxport-cli-import-test-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let nested = base.join("playlist-one/subfolder");
        fs::create_dir_all(&nested).unwrap();
        let first = nested.join("track-a.mp3");
        let second = base.join("playlist-two/track-b.mp3");
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, b"fake mp3").unwrap();
        fs::write(&second, b"fake mp3").unwrap();

        let files = expand_import_paths(&[base.clone()]);
        assert!(files.iter().any(|p| p == &first));
        assert!(files.iter().any(|p| p == &second));

        let _ = fs::remove_dir_all(base);
    }
}
