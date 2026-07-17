use fxhash::{FxHashMap, FxHashSet};
use promwrite::Metric;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

struct FileState {
    path: PathBuf,
    dir_label: String,
    file_label: String,
    seen: bool,
    is_deleted: bool,
}

fn track_files_change<P: AsRef<Path>>(
    root_path: P,
    client: &Metric,
    ext_labels: &[(String, String)],
    min_size: Option<u64>,
    cache: &mut FxHashMap<u64, FileState>,
) -> io::Result<()> {
    let total_items = cache.len();
    let throttle_batch = if total_items < 10000 {
        0
    } else {
        (total_items / 200).max(100)
    };

    for state in cache.values_mut() {
        state.seen = false;
    }

    let base_metric = client.name("fs_telemetry_bytes").labels(ext_labels);

    let mut processed_inodes: FxHashSet<u64> =
        FxHashSet::with_capacity_and_hasher(
            total_items.max(64),
            Default::default(),
        );

    let walker = WalkDir::new(root_path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok());

    let mut scanned_count = 0;

    for entry in walker {
        if !entry.file_type().is_file() {
            continue;
        }

        scanned_count += 1;
        if throttle_batch > 0 && scanned_count % throttle_batch == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }

        let metadata = match entry.metadata() {
            Ok(meta) => meta,
            Err(_) => continue,
        };

        let size_bytes = metadata.len();
        if let Some(min) = min_size {
            if size_bytes < min {
                continue;
            }
        }

        let size = size_bytes as f64;
        let path = entry.path();
        let inode = metadata.ino();

        let is_new_in_this_cycle = processed_inodes.insert(inode);

        match cache.entry(inode) {
            std::collections::hash_map::Entry::Occupied(mut occupied) => {
                let state = occupied.get_mut();
                state.seen = true;
                state.is_deleted = false;

                if state.path != path {
                    if is_new_in_this_cycle
                        && !state.dir_label.is_empty()
                        && !state.file_label.is_empty()
                    {
                        base_metric
                            .clone()
                            .label("dir", &state.dir_label)
                            .label("file", &state.file_label)
                            .set(0.0);
                    }

                    state.path = path.to_path_buf();
                    state.dir_label = path
                        .parent()
                        .unwrap_or(path)
                        .to_string_lossy()
                        .into_owned();
                    state.file_label = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                }

                if is_new_in_this_cycle
                    && !state.dir_label.is_empty()
                    && !state.file_label.is_empty()
                {
                    base_metric
                        .clone()
                        .label("dir", &state.dir_label)
                        .label("file", &state.file_label)
                        .set(size);
                }
            }
            std::collections::hash_map::Entry::Vacant(vacant) => {
                let dir_label = path
                    .parent()
                    .unwrap_or(path)
                    .to_string_lossy()
                    .into_owned();
                let file_label = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();

                let state = vacant.insert(FileState {
                    path: path.to_path_buf(),
                    dir_label,
                    file_label,
                    seen: true,
                    is_deleted: false,
                });

                if is_new_in_this_cycle
                    && !state.dir_label.is_empty()
                    && !state.file_label.is_empty()
                {
                    base_metric
                        .clone()
                        .label("dir", &state.dir_label)
                        .label("file", &state.file_label)
                        .set(size);
                }
            }
        }
    }

    cache.retain(|_, state| {
        if !state.seen {
            if !state.is_deleted {
                if !state.dir_label.is_empty() && !state.file_label.is_empty() {
                    base_metric
                        .clone()
                        .label("dir", &state.dir_label)
                        .label("file", &state.file_label)
                        .set(0.0);
                }
                state.is_deleted = true;
                true
            } else {
                false
            }
        } else {
            true
        }
    });

    Ok(())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();

    let addr = aflag::flag("endpoint", "prometheus remote write api endpoint")
        .default("http://127.0.0.1:9090/api/v1/write");
    let target_dirs = aflag::flag("dir", "target directories to scan")
        .short('d')
        .tooltip()
        .required::<Vec<String>>();
    let interval_secs =
        aflag::flag("interval", "directory scan interval in seconds")
            .short('i')
            .default(15u64);
    let labels_flag = aflag::flag("labels", "external static labels to append")
        .default(Vec::<String>::new());
    let size_flag = aflag::flag(
        "size",
        "only scan files larger than this size (e.g., 10G)",
    )
    .default(None);

    aflag::parse_with_usage(
        r#"file system telemetry exporter.

metrics:
  - fs_telemetry_bytes (gauge): tracks individual file sizes in bytes.

grafana / promql recommendation:
  sum((fs_telemetry_bytes - fs_telemetry_bytes @ start()) != 0) by (dir, file)
"#,
    );

    let current_interval = interval_secs.get();
    let dirs = target_dirs.get();
    let min_size = size_flag.get();

    let paths: Vec<PathBuf> = dirs.iter().map(PathBuf::from).collect();
    let paths_arc = Arc::new(paths);

    let parsed_ext_labels: Vec<(String, String)> = labels_flag
        .get()
        .into_iter()
        .filter_map(|s| {
            let mut parts = s.splitn(2, '=');
            let k = parts.next()?.trim().to_string();
            let v = parts.next()?.trim().to_string();
            if k.is_empty() || v.is_empty() {
                None
            } else {
                Some((k, v))
            }
        })
        .collect();

    let parsed_ext_labels = Arc::new(parsed_ext_labels);
    let client = Arc::new(Metric::new(addr.get()));

    let mut interval =
        tokio::time::interval(Duration::from_secs(current_interval));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut multi_file_cache: FxHashMap<PathBuf, FxHashMap<u64, FileState>> =
        FxHashMap::with_capacity_and_hasher(
            paths_arc.len(),
            Default::default(),
        );
    for p in paths_arc.iter() {
        multi_file_cache.insert(
            p.clone(),
            FxHashMap::with_capacity_and_hasher(16384, Default::default()),
        );
    }

    tracing::info!(
        "🚀 Telemetry Agent active. Dirs: {}, Interval: {}s",
        paths_arc.len(),
        current_interval
    );

    loop {
        interval.tick().await;
        let start_time = Instant::now();

        let labels_arc = Arc::clone(&parsed_ext_labels);
        let client_arc = Arc::clone(&client);
        let paths_shared = Arc::clone(&paths_arc);

        let mut current_caches = std::mem::take(&mut multi_file_cache);

        let join_result = tokio::task::spawn_blocking(move || {
            let mut total_active_items = 0;
            for path_key in paths_shared.iter() {
                if let Some(cache) = current_caches.get_mut(path_key) {
                    let _ = track_files_change(
                        path_key,
                        &client_arc,
                        &labels_arc,
                        min_size,
                        cache,
                    );
                    total_active_items += cache.len();
                }
            }
            (total_active_items, current_caches)
        })
        .await;

        if let Ok((total_items, processed_caches)) = join_result {
            multi_file_cache = processed_caches;
            tracing::info!(
                "Cycle finished. Total Items: {}, cost: {:?}",
                total_items,
                start_time.elapsed()
            );
        }
    }
}
