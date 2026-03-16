# KATCP Performance Benchmarks: Rust vs Python

Comparative benchmark results for `katcp-rust`, `katcp-python` (v0.9.3), and `aiokatcp` (v2.2.1) running on the same machine.

**Test Environment:**
- macOS Darwin 25.3.0 (Apple Silicon)
- Rust: release mode (`cargo test --release`)
- katcp-python: CPython 3.6.15, katcp 0.9.3, Tornado 4.5.3
- aiokatcp: CPython 3.9.6, aiokatcp 2.2.1, asyncio (stdlib)
- All tests run locally over loopback (127.0.0.1)

**Methodology:**
- All three suites use identical iteration counts and workloads
- Each uses its idiomatic async client API (Rust async client, katcp-python `CallbackClient` + `future_request`, aiokatcp async `Client`)
- No artificial rate-limiting — push at natural speed, measure what arrives
- Sensor delivery counters reset after subscription settles to avoid counting setup informs
- Latency percentiles (p50/p95/p99) measured per-request

---

## Summary

| Benchmark | katcp-rust | aiokatcp | katcp-python | Rust vs aiokatcp | Rust vs katcp-python |
|---|---|---|---|---|---|
| Watchdog request/reply | 18.64K/s | 10.74K/s | 4.73K/s | **1.7x** | **3.9x** |
| Sensor-value (single) | 14.25K/s | 9.58K/s | 3.58K/s | **1.5x** | **4.0x** |
| Sensor-value (all 10) | 11.29K/s | 6.74K/s | 1.63K/s | **1.7x** | **6.9x** |
| Help request | 11.81K/s | 7.51K/s | 429/s | **1.6x** | **27.5x** |
| Concurrent clients (10) | 70.64K/s | 24.45K/s | 5.05K/s | **2.9x** | **14.0x** |
| Sensor delivery (10K push) | 2.23M/s | 123.2K/s | 33.5K/s | **18.1x** | **66.6x** |
| Multi-sensor delivery (50 sensors) | 2.32M/s | 99.4K/s | 38.1K/s | **23.3x** | **60.9x** |
| 1000 sensors: sensor-list | 561/s | 154/s | 24.9/s | **3.6x** | **22.5x** |
| 1000 sensors: sensor-value (all) | 536/s | 177/s | 25.8/s | **3.0x** | **20.8x** |
| 1000 sensors: sensor-value (single) | 26.40K/s | 9.39K/s | 2.44K/s | **2.8x** | **10.8x** |
| 1000 sensors: sensor-list (regex) | 4.87K/s | 1.19K/s | 213/s | **4.1x** | **22.9x** |
| 20K sensors: subscribe rate | 36.94K/s | 7.18K/s | 2.81K/s | **5.1x** | **13.1x** |
| 20K sensors: push rate | 1.97M/s | 137.7K/s | 36.1K/s | **14.3x** | **54.6x** |

---

## Detailed Results

### Request/Reply Latency

Single client issuing sequential requests and waiting for replies. 100 warmup iterations excluded.

| Request Type | katcp-rust | aiokatcp | katcp-python |
|---|---|---|---|
| `?watchdog` (2000 iters) | 18,640/s (p50=50us, p95=87us, p99=102us) | 10,740/s (p50=88us, p95=121us, p99=136us) | 4,730/s (p50=207us, p95=237us, p99=304us) |
| `?sensor-value` single (2000 iters) | 14,250/s (p50=67us, p95=105us, p99=152us) | 9,580/s (p50=100us, p95=128us, p99=163us) | 3,580/s (p50=273us, p95=315us, p99=382us) |
| `?sensor-value` all 10 (1000 iters) | 11,290/s (p50=84us, p95=138us, p99=166us) | 6,740/s (p50=141us, p95=178us, p99=203us) | 1,630/s (p50=585us, p95=752us, p99=861us) |
| `?help` (1000 iters) | 11,810/s (p50=80us, p95=139us, p99=161us) | 7,510/s (p50=128us, p95=155us, p99=184us) | 429/s (p50=2,310us, p95=2,556us, p99=2,726us) |

- Rust achieves ~50us round-trip latency, aiokatcp ~88us, and katcp-python ~207us.
- aiokatcp is 2.3x faster than katcp-python for simple requests, using asyncio vs Tornado.
- The katcp-python gap widens dramatically for multi-inform responses (`?help`: 429/s vs 7,510/s for aiokatcp). aiokatcp uses `katcp-codec` (a Rust-based parser via PyO3) which eliminates much of the per-message Python overhead.
- aiokatcp's `?help` is nearly as fast as simpler requests (7.5K/s vs 10.7K/s) while katcp-python drops from 4.7K/s to 429/s — an 11x cliff.

### Sensor Delivery

Server pushes sensor updates at natural (unthrottled) speed; client receives `#sensor-status` informs via event sampling.

| Metric | katcp-rust | aiokatcp | katcp-python |
|---|---|---|---|
| Single sensor, 10K updates push rate | 2.23M/s | 123.2K/s | 33.5K/s |
| Single sensor, 10K delivery | 100% | 100% | 100% |
| 50 sensors, 200 updates each push rate | 2.32M/s | 99.4K/s | 38.1K/s |
| 50 sensors, 10K total delivery | 100% | 100% | 100% |

- All three implementations deliver 100% of updates at natural push rate.
- aiokatcp pushes 3.7x faster than katcp-python for single sensor updates, and 2.6x faster for multi-sensor.
- Rust remains ~18x faster than aiokatcp for sensor push throughput — the difference is in-process sensor update + observer dispatch overhead.

### Concurrent Client Throughput

10 clients issuing 500 `?watchdog` requests each simultaneously.

| Metric | katcp-rust | aiokatcp | katcp-python |
|---|---|---|---|
| Aggregate throughput | 70,640/s | 24,450/s | 5,050/s |
| Per-client rate | 7,064/s | 2,445/s | 505/s |

- aiokatcp's asyncio event loop handles concurrent clients 4.8x faster than katcp-python's Tornado-based async approach.
- Rust's tokio multi-threaded runtime achieves 2.9x higher throughput than aiokatcp's single-threaded asyncio loop.

### High Sensor Count (1,000 sensors)

Performance with 1,000 registered sensors.

| Operation | katcp-rust | aiokatcp | katcp-python |
|---|---|---|---|
| `?sensor-list` (all, 50 iters) | 561/s | 154/s | 24.9/s |
| `?sensor-value` (all, 50 iters) | 536/s | 177/s | 25.8/s |
| `?sensor-value` (single, 1000 iters) | 26,400/s | 9,390/s | 2,440/s |
| `?sensor-list` (regex, 200 iters) | 4,870/s | 1,190/s | 213/s |

- Rust is 3.6x faster than aiokatcp and 22.5x faster than katcp-python for listing all 1,000 sensors.
- Single sensor lookups: Rust (26.4K/s) vs aiokatcp (9.4K/s) vs katcp-python (2.4K/s).
- Regex filtering: Rust (4.9K/s) vs aiokatcp (1.2K/s) vs katcp-python (213/s) — Rust is 4.1x faster than aiokatcp and 22.9x faster than katcp-python.

### 20,000 Unique Sensors

End-to-end test with 20K distinct sensors, event sampling on all, 3 rounds of updates (60K total).

| Metric | katcp-rust | aiokatcp | katcp-python |
|---|---|---|---|
| Subscribe all 20K | 0.541s (36.94K/s) | 2.786s (7.18K/s) | 7.128s (2.81K/s) |
| Push 60K updates | 0.030s (1.97M/s) | 0.436s (137.7K/s) | 1.663s (36.1K/s) |
| Client delivery | 100.0% | 100.0% | 100.0% |
| `?sensor-list` (all 20K) | 0.044s | 0.154s | 1.312s |
| `?sensor-value` (all 20K) | 0.028s | 0.147s | 0.985s |

- All three achieve 100% delivery of 60K updates.
- aiokatcp subscribes 2.6x faster and pushes 3.8x faster than katcp-python.
- For bulk queries, aiokatcp is 8.5x faster for `?sensor-list` and 6.7x faster for `?sensor-value` than katcp-python.
- Rust remains the fastest across all metrics: 5.1x faster than aiokatcp for subscriptions, 14.3x faster for push throughput.

---

## Architectural Differences Driving Performance

| Aspect | katcp-rust | aiokatcp | katcp-python |
|---|---|---|---|
| Runtime | tokio (multi-threaded async) | asyncio (single-threaded async) | Tornado (single-threaded event loop) |
| Concurrency | True parallelism via OS threads + async tasks | Cooperative multitasking in one thread | Cooperative multitasking in one thread |
| Client API | Async (native) | Async (native) | Async (`future_request` on Tornado IOLoop) |
| Sensor locking | `std::sync::Mutex` (fast spinlock) | Python `asyncio` task scheduling | Python `threading.Lock` + GIL |
| Observer dispatch | Direct `mpsc::UnboundedSender` channel | asyncio callback scheduling | `ioloop.add_callback()` scheduling |
| Message parsing | Zero-copy byte slicing | `katcp-codec` (Rust via PyO3) | Pure Python string/bytes allocation |
| Memory | Stack-allocated messages, arena-style buffers | Heap-allocated Python objects | Heap-allocated Python objects |
| Python version | N/A | 3.8+ | 2.7+ / 3.x |

### Why aiokatcp is faster than katcp-python

1. **Native asyncio vs Tornado**: aiokatcp uses Python's built-in `asyncio` which has lower overhead than Tornado's older event loop abstraction.
2. **Rust-based parser**: aiokatcp uses `katcp-codec`, a Rust-compiled message parser exposed via PyO3, eliminating the pure-Python parsing bottleneck that dominates katcp-python's multi-inform response handling.
3. **Modern Python**: aiokatcp runs on Python 3.8+ with significant interpreter improvements over the Python 3.6 required by katcp-python.

### Why katcp-rust is faster than aiokatcp

1. **Multi-threaded runtime**: tokio distributes work across CPU cores; asyncio runs in a single thread.
2. **Zero-copy message handling**: Rust processes messages as byte slices without heap allocation, while even `katcp-codec` must cross the Python/Rust FFI boundary and allocate Python objects.
3. **Lock-free sensor updates**: Rust's `Mutex` is a lightweight spinlock with no GIL contention, whereas aiokatcp still runs within the Python GIL.
4. **Compiled dispatch**: Request handlers in Rust are compiled to native code with zero dynamic dispatch overhead.
