# fs-telemetry-exporter

A high-performance, lightweight telemetry agent that scans filesystem directories and streams file metadata directly to Prometheus via Remote Write 2.0 (`fs_telemetry_bytes`).

---

## ✨ Features & Architecture

* **⚡ Low Overhead & Zero Allocation**: Built on top of `promwrite` and `jemalloc`, ensuring predictable heap usage (~1–2 MB) and minimal CPU consumption (< 0.2%) under production database workloads.
* **🛡️ Inode-Level Tracking**: Tracks files using filesystem inodes (`st_ino`). Seamlessly handles file renames and moves across monitored directories.
* **🧹 Automatic Stale Marker Cleanups**: Automatically emits Prometheus Remote Write stale markers (`.del()`) when files are deleted or purged from the filesystem.
* **🏷️ Rich Label Management**: Dynamically extracts standard `dir` and `file` dimensions while seamlessly merging user-defined static labels (e.g., `hostname`, `env`).

---

## 📊 Metrics

| Metric | Type | Description |
| :--- | :--- | :--- |
| `fs_telemetry_bytes` | **Gauge** | Tracks individual file sizes in bytes with `dir` and `file` labels. |
| `fs_metadata_duration_seconds` | **Gauge** | Tracks max filesystem metadata read duration in seconds per target directory. |

---

## 📈 PromQL

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

### Usage

Run the exporter to monitor large database files (e.g., files > 10 GiB) and push to a remote Prometheus/VictoriaMetrics endpoint:

```bash
fs-telemetry-exporter \
    --endpoint http://user:pwd@192.168.168.168:9090/api/v1/write \
    --dir /data/db/data \
    --size 10G \
    --labels hostname=mysql002
```

---

### Output Logs
```text
2026-07-23T03:55:28.857106Z  INFO fs_telemetry_exporter: Cycle finished. Total Items: 26745, cost: 1.262968706s
2026-07-23T03:55:33.738516Z  INFO fs_telemetry_exporter: Cycle finished. Total Items: 26745, cost: 1.252980744s
```