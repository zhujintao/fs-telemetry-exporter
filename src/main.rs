use fxhash::{FxHashMap, FxHashSet};
use promwrite::{Metric, MetricBuilder};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

struct FileState {
    dir_label: Arc<str>,
    file_label: Arc<str>,
}

fn track_files_change<P: AsRef<Path>>(
    root_path: P,
    base_metric: &MetricBuilder,
    meta_metric: &MetricBuilder,
    min_size: Option<u64>,
    cache: &mut FxHashMap<u64, FileState>,
) -> io::Result<()> {
    let root = root_path.as_ref();
    let root_str = root.to_string_lossy();

    let walker = WalkDir::new(root)
        .follow_links(false)
        .same_file_system(true)
        .into_iter()
        .filter_map(|e| e.ok());

    let total_cache_size = cache.len();

    let mut current_inodes = FxHashSet::with_capacity_and_hasher(
        total_cache_size,
        Default::default(),
    );

    let mut max_meta_cost_secs = 0.0f64;

    for entry in walker {
        if !entry.file_type().is_file() {
            continue;
        }

        let meta_start = Instant::now();
        let metadata = match entry.metadata() {
            Ok(m) => {
                let cost_secs = meta_start.elapsed().as_secs_f64();
                if cost_secs > max_meta_cost_secs {
                    max_meta_cost_secs = cost_secs;
                }
                m
            }
            Err(_) => continue,
        };

        let size_bytes = metadata.len();
        if let Some(min) = min_size {
            if size_bytes < min {
                continue;
            }
        }

        let inode = metadata.ino();
        current_inodes.insert(inode);
        let size = size_bytes as f64;

        match cache.entry(inode) {
            std::collections::hash_map::Entry::Occupied(occupied) => {
                let state = occupied.get();

                base_metric
                    .clone()
                    .label("dir", &state.dir_label)
                    .label("file", &state.file_label)
                    .set(size);
            }
            std::collections::hash_map::Entry::Vacant(vacant) => {
                let path = entry.path();
                let (d_str, f_str) = extract_labels(path);

                base_metric
                    .clone()
                    .label("dir", d_str.as_ref())
                    .label("file", f_str.as_ref())
                    .set(size);

                vacant.insert(FileState {
                    dir_label: d_str,
                    file_label: f_str,
                });
            }
        }
    }

    meta_metric
        .clone()
        .label("target_dir", &root_str)
        .set(max_meta_cost_secs);

    cache.retain(|inode, state| {
        let keep = current_inodes.contains(inode);
        if !keep {
            base_metric
                .clone()
                .label("dir", &state.dir_label)
                .label("file", &state.file_label)
                .del();
        }
        keep
    });

    Ok(())
}

#[inline]
fn extract_labels(path: &Path) -> (Arc<str>, Arc<str>) {
    let dir_str = path
        .parent()
        .unwrap_or(path)
        .to_str()
        .map(Arc::from)
        .unwrap_or_else(|| {
            Arc::from(path.parent().unwrap_or(path).to_string_lossy().as_ref())
        });

    let file_str = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(Arc::from)
        .unwrap_or_else(|| {
            Arc::from(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref(),
            )
        });

    (dir_str, file_str)
}

promwrite::use_jemalloc!();

fn main() {
    tracing_subscriber::fmt::init();

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
  - fs_metadata_duration_seconds (gauge): tracks metadata read duration in seconds.

promql:
  sum((fs_telemetry_bytes - fs_telemetry_bytes @ start()) != 0) by (dir, file)"#,
    );

    let current_interval = Duration::from_secs(interval_secs.get());
    let dirs = target_dirs.get();
    let min_size = size_flag.get();

    let paths: Vec<PathBuf> = dirs.iter().map(PathBuf::from).collect();

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

    let client = Metric::new(addr.get());
    let base_metric =
        client.name("fs_telemetry_bytes").labels(&parsed_ext_labels);

    let meta_metric = client
        .name("fs_metadata_duration_seconds")
        .labels(&parsed_ext_labels);

    let mut multi_file_cache: FxHashMap<PathBuf, FxHashMap<u64, FileState>> =
        FxHashMap::with_capacity_and_hasher(paths.len(), Default::default());

    for p in paths.iter() {
        multi_file_cache.insert(
            p.clone(),
            FxHashMap::with_capacity_and_hasher(64, Default::default()),
        );
    }

    loop {
        let start_time = Instant::now();
        let mut total_active_items = 0;

        for path_key in paths.iter() {
            if let Some(cache) = multi_file_cache.get_mut(path_key) {
                let _ = track_files_change(
                    path_key,
                    &base_metric,
                    &meta_metric,
                    min_size,
                    cache,
                );
                total_active_items += cache.len();
            }
        }

        let elapsed = start_time.elapsed();
        tracing::info!(
            "Cycle finished. Total Items: {}, cost: {:?}",
            total_active_items,
            elapsed
        );

        promwrite::purge_heap();

        if let Some(remaining) = current_interval.checked_sub(elapsed) {
            std::thread::sleep(remaining);
        }
    }
}
