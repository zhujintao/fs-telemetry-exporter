# fs-telemetry-exporter

A lightweight CLI agent that scans directories and streams file size changes as Prometheus gauges (`fs_telemetry_bytes`) via Remote Write API.

## 🛠️ Architecture Details

* **Inode-Level Tracking**: Identifies files by `st_ino` (inode number). Automatically re-maps labels when a file is renamed or moved across monitored directories, and emits `0.0` upon file deletion.
* **Scan Throttling**: Automatically injects a 5ms sleep per batch once the file cache exceeds 10,000 items to bound disk IO and CPU usage during deep directory traversals.
* **Label Management**: Appends dynamic filesystem dimensions (`dir`, `file`) alongside optional external static labels passed via CLI arguments.

## 📊 Metrics

* `fs_telemetry_bytes` (Gauge): Tracks individual file sizes in bytes.

## 📈 PromQL

To detect active file size variations within the query range:

```promql
sum((fs_telemetry_bytes - fs_telemetry_bytes @ start()) != 0) by (dir, file)
```
## 🚀 Quick Start
Run the exporter to monitor large database files (e.g., files > 10 GiB) and push to a remote Prometheus/VictoriaMetrics endpoint:
```bash
fs-telemetry-exporter \
    --endpoint http://user:pwd@192.168.168.168:9090/api/v1/write \
    --dir /data/db/data \
    --size 10G \
    --labels hostname=mysql002
```
### Output Logs
```text
2026-07-17T09:05:27.521309Z  INFO fs_telemetry_exporter: 🚀 Telemetry Agent active. Dirs: 1, Interval: 15s
2026-07-17T09:05:27.530400Z  INFO fs_telemetry_exporter: Cycle finished. Total Items: 35, cost: 8.004598ms
```