use fxhash::{FxHashMap, FxHashSet};
use promwrite::{Metric, MetricBuilder, MetricConfig};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::Level;
use walkdir::WalkDir;

struct FileState {
    metric_handle: MetricBuilder,
}

struct DirContext {
    cache: FxHashMap<u64, FileState>,
    current_inodes: FxHashSet<u64>,
    meta_metric: MetricBuilder,
    total_files_metric: MetricBuilder,
}

fn track_files_change<P: AsRef<Path>>(
    root_path: P,
    base_metric: &MetricBuilder,
    ctx: &mut DirContext,
    min_size: Option<u64>,
) -> io::Result<()> {
    let root = root_path.as_ref();

    let walker = WalkDir::new(root)
        .follow_links(false)
        .same_file_system(true)
        .into_iter()
        .filter_map(|e| e.ok());

    ctx.current_inodes.clear();

    let mut max_meta_cost_secs = 0.0f64;
    let mut total_scanned_files = 0u64;

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

        total_scanned_files += 1;

        let size_bytes = metadata.len();
        if let Some(min) = min_size {
            if size_bytes < min {
                continue;
            }
        }

        let inode = metadata.ino();
        ctx.current_inodes.insert(inode);
        let size = size_bytes as f64;

        ctx.cache
            .entry(inode)
            .and_modify(|state| {
                state.metric_handle.set(size);
            })
            .or_insert_with(|| {
                let path = entry.path();
                let (d_str, f_str) = extract_labels_str(path);

                let mut metric_handle = base_metric
                    .clone()
                    .label("dir", d_str)
                    .label("file", f_str);

                metric_handle.set(size);
                FileState { metric_handle }
            });
    }

    ctx.total_files_metric.set(total_scanned_files as f64);
    ctx.meta_metric.set(max_meta_cost_secs);

    let current_inodes = &ctx.current_inodes;
    ctx.cache.retain(|inode, state| {
        let keep = current_inodes.contains(inode);
        if !keep {
            state.metric_handle.del();
        }
        keep
    });

    Ok(())
}

#[inline(always)]
fn extract_labels_str(path: &Path) -> (&str, &str) {
    let parent_path = path.parent().unwrap_or(path);
    let dir_str = parent_path.to_str().unwrap_or("");

    let file_name = path.file_name().unwrap_or_default();
    let file_str = file_name.to_str().unwrap_or("");

    (dir_str, file_str)
}

promwrite::use_jemalloc!();

fn main() {
    let addr =
        aflag::flag("endpoint", "prometheus\nremote write api endpoint.")
            .env("FS_PROMWRITE_ENDPOINT")
            .default("http://127.0.0.1:9090/api/v1/write");
    let target_dirs = aflag::flag("dir", "target directories to scan.")
        .short('d')
        .tooltip()
        .required::<Vec<String>>();
    let interval_secs =
        aflag::flag("interval", "directory scan interval in seconds.")
            .short('i')
            .default(15u64);
    let labels_flag = aflag::flag(
        "labels",
        "external static labels to append. e.g., name=haha,env=prod",
    )
    .default(Vec::<String>::new());
    let size_flag =
        aflag::flag("size", "only scan files larger than this size. e.g., 10G")
            .default(None::<u64>);

    aflag::enable_version!();
    aflag::parse_with_usage(
        r#"file system telemetry exporter.

metrics:
  - fs_telemetry_bytes (gauge): tracks individual file sizes in bytes.
  - fs_metadata_duration_seconds (gauge): tracks metadata read duration in seconds.
  - fs_telemetry_scanned_files_total (gauge): tracks total files scanned before size filtering.

promql:
  sum((fs_telemetry_bytes - fs_telemetry_bytes @ start()) != 0) by (dir, file)"#,
    );

    let current_interval = Duration::from_secs(interval_secs.get());
    let dirs = target_dirs.get();
    let min_size = size_flag.get();

    println!(
        "target dir : {:?} labels: {:?}",
        target_dirs,
        labels_flag.get()
    );
    tracing_subscriber::fmt()
        .with_max_level(Level::ERROR)
        .init();

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

    let client = Metric::with_config(MetricConfig {
        url: addr.get(),
        ..Default::default()
    });

    let base_metric =
        client.name("fs_telemetry_bytes").labels(&parsed_ext_labels);

    let mut multi_file_cache: FxHashMap<PathBuf, DirContext> =
        FxHashMap::with_capacity_and_hasher(paths.len(), Default::default());

    for p in paths.iter() {
        let root_str = p.to_string_lossy();

        let meta_metric = client
            .name("fs_metadata_duration_seconds")
            .labels(&parsed_ext_labels)
            .label("target_dir", root_str.as_ref());

        let total_files_metric = client
            .name("fs_telemetry_scanned_files_total")
            .labels(&parsed_ext_labels)
            .label("target_dir", root_str.as_ref());

        multi_file_cache.insert(
            p.clone(),
            DirContext {
                cache: FxHashMap::with_capacity_and_hasher(
                    1024,
                    Default::default(),
                ),
                current_inodes: FxHashSet::with_capacity_and_hasher(
                    1024,
                    Default::default(),
                ),
                meta_metric,
                total_files_metric,
            },
        );
    }

    loop {
        let start_time = Instant::now();

        for path_key in paths.iter() {
            if let Some(ctx) = multi_file_cache.get_mut(path_key) {
                let _ =
                    track_files_change(path_key, &base_metric, ctx, min_size);
            }
        }

        let elapsed = start_time.elapsed();

        promwrite::purge_heap();

        if let Some(remaining) = current_interval.checked_sub(elapsed) {
            std::thread::sleep(remaining);
        }
    }
}
