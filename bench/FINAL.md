# ZCode Node vs Rust — final comparison (2026-09-22 01:58, M-series, n=20+

## Latency (mean ms, lower is better)
| command | node_ms | rust_ms | speedup |
|---|---|---|---|
| --version | 369.6 | 1.5 | 252x |
| --help | 378.0 | 1.6 | 239x |
| doctor | 365.8 | 1.7 | 220x |
| commands list | 376.1 | 1.7 | 216x |
| skills list | 375.9 | 2.0 | 187x |
| plugins list | 371.6 | 1.5 | 250x |

## Peak RSS (MB, lower is better)
| command | node_mb | rust_mb | reduction |
|---|---|---|---|
| --version | 367 | 1 | 226x |
| doctor | 367 | 1 | 224x |
| skills list | 371 | 2 | 142x |
| plugins list | 371 | 1 | 191x |
