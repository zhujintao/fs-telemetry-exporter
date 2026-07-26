# fs-telemetry-exporter

> A high-performance, zero-allocation filesystem telemetry agent that streams file metadata directly to Prometheus via Remote Write 2.0.

Powered by **`promwrite`** for zero-allocation metric streaming and **`aflag`** for type-safe CLI flag parsing.

---

## ✨ Features & Architecture

* **⚡ Pre-bound Zero Allocation Hot Path**: Built on top of `promwrite 0.2.3` with pre-bound metric handles. Hot path updates execute with **0 heap allocations** and near-zero CPU overhead.
* **🚩 Clean & Type-Safe CLI**: Powered by `aflag` with native environment variable fallbacks (`FS_PROMWRITE_ENDPOINT`), auto-generated usage help, and human-friendly size parsing (e.g., `--size 10G`).
* **🛡️ Inode-Level Tracking**: Tracks files using filesystem inodes (`st_ino`). Seamlessly handles file renames and moves across monitored directories without metric cardinality duplication.
* **🧹 Automatic Stale Series Cleanup**: Automatically dispatches Prometheus Remote Write stale markers (`.del()`) when files are deleted or purged from disk to release remote TSDB memory.
* **🏷️ Static & Dynamic Labeling**: Automatically extracts `dir` and `file` dimensions while appending user-defined static labels (e.g., `hostname`, `env`).
* **📦 Minimal Memory Footprint**: Integrated with `jemalloc` explicit purging and lightweight `tracing-subscriber` without heavy regex dynamic filters, maintaining a tiny memory footprint (~1–2 MB RSS).

---

## 📊 Metrics

| Metric | Type | Description |
| :--- | :--- | :--- |
| `fs_telemetry_bytes` | **Gauge** | Tracks individual file sizes in bytes with `dir` and `file` labels. |
| `fs_metadata_duration_seconds` | **Gauge** | Tracks max filesystem metadata (`stat`) read duration in seconds per target directory. |
| `fs_telemetry_scanned_files_total` | **Gauge** | Tracks total scanned file counts per target directory before size filtering. |

---

## 📈 PromQL Examples

### 1. Total Disk Usage by Directory
Calculate the aggregated size of all tracked files for each directory:
```promql
sum(fs_telemetry_bytes) by (dir)
```

### 2. Detect Active File Size Variations Within Range
Count the number of files that have experienced size changes (either grown or shrunk) between the start and end of the selected dashboard time range:
```promql
sum((fs_telemetry_bytes - fs_telemetry_bytes @ start()) != 0) by (dir, file)
```
> 💡 **Note**: `@ start()` calculates size changes relative to the dashboard range start (e.g., set Grafana **Relative time** to `now/d` to track changes since midnight).

---

## 🚀 Quick Start

### Command Line Flags (`aflag`)

| Flag | Short | Environment Variable | Default | Description |
| :--- | :--- | :--- | :--- | :--- |
| `--endpoint` | | `FS_PROMWRITE_ENDPOINT` | `http://127.0.0.1:9090/api/v1/write` | Prometheus Remote Write API Endpoint. |
| `--dir` | `-d` | | *(Required)* | Target directories to scan (supports multiple `-d`). |
| `--interval` | `-i` | | `15` | Directory scan interval in seconds. |
| `--labels` | | | `[]` | External static labels (e.g., `hostname=db01,env=prod`). |
| `--size` | | | `None` | Minimum file size threshold filter (e.g., `10G`). |
| `--log-level` | | | `error` | Log level: `debug`, `info`, or `error`. |

### Usage Example

Run the exporter to monitor large database files (> 10 GiB) and push to a remote VictoriaMetrics / Prometheus endpoint:

```bash
fs-telemetry-exporter \
    --endpoint [http://admin:secret@192.168.1.100:9090/api/v1/write](http://admin:secret@192.168.1.100:9090/api/v1/write) \
    -d /data/db/data \
    -i 10 \
    --size 10G \
    --labels hostname=mysql002,env=prod
```
Or set the endpoint via environment variable:

```bash
export FS_PROMWRITE_ENDPOINT="http://192.168.1.100:9090/api/v1/write"

fs-telemetry-exporter -d /data/db/data --size 10G
```

### Console Output

By default, the exporter runs **completely silent** during normal operation to minimize terminal I/O overhead. Terminal logs are only triggered when explicit errors occur (e.g., remote write failure or network partition):

```text
2026-07-26T05:49:50.239327Z ERROR promwrite: Failed to send remote write payload (network error) error=io: Connection refused url=http://127.0.0.1:9090/api/v1/write
2026-07-26T05:49:50.312793Z ERROR promwrite: Failed to send remote write payload (network error) error=io: Connection refused url=http://127.0.0.1:9090/api/v1/write
2026-07-26T05:49:50.389023Z ERROR promwrite: Failed to send remote write payload (network error) error=io: Connection refused url=http://127.0.0.1:9090/api/v1/write
```