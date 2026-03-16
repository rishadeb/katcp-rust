//! Performance benchmarks for the KATCP Rust implementation.
//!
//! Run with: cargo test --release -p katcp --test benchmarks -- --nocapture
//!
//! These are end-to-end benchmarks over TCP, designed to be directly
//! comparable with the matching katcp-python benchmarks. Both suites use:
//!   - Identical iteration counts and sensor counts
//!   - Idiomatic API usage for each language
//!   - Same warmup procedures
//!   - Latency percentiles (p50/p95/p99) for request tests
//!   - Fixed-N push + drain for delivery tests (no artificial throttling)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use katcp::client::DeviceClient;
use katcp::sensor::{Sensor, SensorStatus};
use katcp::server::DeviceServer;

// ---------------------------------------------------------------------------
// Constants — shared with the Python suite
// ---------------------------------------------------------------------------

const WARMUP: usize = 100;

const REQ_ITERATIONS: u64 = 2_000;
const HELP_ITERATIONS: u64 = 1_000;
const CONCURRENT_CLIENTS: usize = 10;
const CONCURRENT_REQS_PER_CLIENT: u64 = 500;

const DELIVERY_UPDATES: u64 = 10_000; // fixed-N for delivery tests
const MULTI_SENSOR_COUNT: usize = 50;
const MULTI_SENSOR_UPDATES_PER: u64 = 200; // 50 * 200 = 10K total

const HIGH_SENSOR_COUNT: usize = 1_000;
const HIGH_SENSOR_LIST_ITERS: u64 = 50;
const HIGH_SENSOR_VALUE_ALL_ITERS: u64 = 50;
const HIGH_SENSOR_VALUE_SINGLE_ITERS: u64 = 1_000;
const HIGH_SENSOR_REGEX_ITERS: u64 = 200;

const SCALE_SENSOR_COUNT: usize = 20_000;
const SCALE_ROUNDS: u64 = 3;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Start a bench server with a given number of sensors.
async fn start_bench_server(
    num_sensors: usize,
) -> (Arc<DeviceServer>, u16, tokio::sync::mpsc::Sender<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let server = Arc::new(DeviceServer::new(
        "127.0.0.1",
        port,
        ("bench-server", 1, 0),
        ("bench-build", 1, 0, "bench"),
    ));

    for i in 0..num_sensors {
        server
            .add_sensor(Sensor::integer(
                &format!("sensor-{}", i),
                "bench sensor",
                "count",
                None,
                None,
                0,
            ))
            .await;
    }

    let (_, shutdown_tx) = server.clone().start();
    tokio::time::sleep(Duration::from_millis(50)).await;
    (server, port, shutdown_tx)
}

/// Connect a client to the bench server.
async fn connect_client(port: u16) -> Arc<DeviceClient> {
    let client = DeviceClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    client
        .wait_protocol(Duration::from_secs(5))
        .await
        .unwrap();
    client
}

/// Format a rate to a human-readable string.
fn format_rate(count: u64, elapsed: Duration) -> String {
    let rate = count as f64 / elapsed.as_secs_f64();
    if rate >= 1_000_000.0 {
        format!("{:.2}M/s", rate / 1_000_000.0)
    } else if rate >= 1_000.0 {
        format!("{:.2}K/s", rate / 1_000.0)
    } else {
        format!("{:.2}/s", rate)
    }
}

/// Calculate percentiles (p50, p95, p99) of latencies.
fn percentiles(latencies: &mut Vec<f64>) -> (f64, f64, f64) {
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = latencies.len();
    let p50 = latencies[n / 2];
    let p95 = latencies[(n as f64 * 0.95) as usize];
    let p99 = latencies[(n as f64 * 0.99) as usize];
    (p50, p95, p99)
}

/// Print latency statistics.
fn print_latency_stats(name: &str, iterations: u64, elapsed: Duration, lats: &mut Vec<f64>) {
    let (p50, p95, p99) = percentiles(lats);
    println!();
    println!("=== {} ===", name);
    println!(
        "  {} requests in {:.3}s = {}",
        iterations,
        elapsed.as_secs_f64(),
        format_rate(iterations, elapsed)
    );
    println!(
        "  Latency: p50={:.0}us  p95={:.0}us  p99={:.0}us",
        p50, p95, p99
    );
}

// ---------------------------------------------------------------------------
// 1. Request latency: ?watchdog
// ---------------------------------------------------------------------------

/// Benchmark watchdog request latency.
#[tokio::test]
async fn bench_01_watchdog_latency() {
    let (_server, port, _shutdown) = start_bench_server(0).await;
    let client = connect_client(port).await;
    let timeout = Duration::from_secs(5);

    for _ in 0..WARMUP {
        client.request_str("watchdog", &[], timeout).await.unwrap();
    }

    let mut latencies = Vec::with_capacity(REQ_ITERATIONS as usize);
    let start = Instant::now();
    for _ in 0..REQ_ITERATIONS {
        let t = Instant::now();
        let (reply, _) = client.request_str("watchdog", &[], timeout).await.unwrap();
        latencies.push(t.elapsed().as_micros() as f64);
        assert!(reply.is_ok());
    }
    let elapsed = start.elapsed();

    print_latency_stats("Watchdog Latency", REQ_ITERATIONS, elapsed, &mut latencies);
    client.disconnect().await;
}

// ---------------------------------------------------------------------------
// 2. Request latency: ?sensor-value (single sensor)
// ---------------------------------------------------------------------------

/// Benchmark sensor value request latency for a single sensor.
#[tokio::test]
async fn bench_02_sensor_value_single_latency() {
    let (server, port, _shutdown) = start_bench_server(10).await;
    let client = connect_client(port).await;
    let timeout = Duration::from_secs(5);

    {
        let sensors = server.context().sensors.read().await;
        for i in 0..10 {
            sensors
                .get(&format!("sensor-{}", i))
                .unwrap()
                .set_int(i as i64, SensorStatus::Nominal);
        }
    }

    for _ in 0..WARMUP {
        client
            .request_str("sensor-value", &["sensor-0"], timeout)
            .await
            .unwrap();
    }

    let mut latencies = Vec::with_capacity(REQ_ITERATIONS as usize);
    let start = Instant::now();
    for _ in 0..REQ_ITERATIONS {
        let t = Instant::now();
        let (reply, informs) = client
            .request_str("sensor-value", &["sensor-0"], timeout)
            .await
            .unwrap();
        latencies.push(t.elapsed().as_micros() as f64);
        assert!(reply.is_ok());
        assert_eq!(informs.len(), 1);
    }
    let elapsed = start.elapsed();

    print_latency_stats(
        "Sensor-Value (single) Latency",
        REQ_ITERATIONS,
        elapsed,
        &mut latencies,
    );
    client.disconnect().await;
}

// ---------------------------------------------------------------------------
// 3. Request latency: ?sensor-value (all 10)
// ---------------------------------------------------------------------------

/// Benchmark sensor value request latency for all sensors.
#[tokio::test]
async fn bench_03_sensor_value_all_latency() {
    let (server, port, _shutdown) = start_bench_server(10).await;
    let client = connect_client(port).await;
    let timeout = Duration::from_secs(5);

    {
        let sensors = server.context().sensors.read().await;
        for i in 0..10 {
            sensors
                .get(&format!("sensor-{}", i))
                .unwrap()
                .set_int(i as i64, SensorStatus::Nominal);
        }
    }

    for _ in 0..WARMUP {
        client
            .request_str("sensor-value", &[], timeout)
            .await
            .unwrap();
    }

    let mut latencies = Vec::with_capacity(HELP_ITERATIONS as usize);
    let start = Instant::now();
    for _ in 0..HELP_ITERATIONS {
        let t = Instant::now();
        let (reply, informs) = client
            .request_str("sensor-value", &[], timeout)
            .await
            .unwrap();
        latencies.push(t.elapsed().as_micros() as f64);
        assert!(reply.is_ok());
        assert_eq!(informs.len(), 10);
    }
    let elapsed = start.elapsed();

    print_latency_stats(
        "Sensor-Value (all 10) Latency",
        HELP_ITERATIONS,
        elapsed,
        &mut latencies,
    );
    client.disconnect().await;
}

// ---------------------------------------------------------------------------
// 4. Request latency: ?help
// ---------------------------------------------------------------------------

/// Benchmark help request latency.
#[tokio::test]
async fn bench_04_help_latency() {
    let (_server, port, _shutdown) = start_bench_server(0).await;
    let client = connect_client(port).await;
    let timeout = Duration::from_secs(5);

    for _ in 0..WARMUP {
        client.request_str("help", &[], timeout).await.unwrap();
    }

    let mut latencies = Vec::with_capacity(HELP_ITERATIONS as usize);
    let start = Instant::now();
    for _ in 0..HELP_ITERATIONS {
        let t = Instant::now();
        let (reply, informs) = client.request_str("help", &[], timeout).await.unwrap();
        latencies.push(t.elapsed().as_micros() as f64);
        assert!(reply.is_ok());
        assert!(informs.len() >= 9);
    }
    let elapsed = start.elapsed();

    print_latency_stats("Help Latency", HELP_ITERATIONS, elapsed, &mut latencies);
    client.disconnect().await;
}

// ---------------------------------------------------------------------------
// 5. Concurrent clients
// ---------------------------------------------------------------------------

/// Benchmark concurrent clients making requests.
#[tokio::test]
async fn bench_05_concurrent_clients() {
    let (_server, port, _shutdown) = start_bench_server(1).await;

    let mut clients = Vec::new();
    for _ in 0..CONCURRENT_CLIENTS {
        clients.push(connect_client(port).await);
    }

    let timeout = Duration::from_secs(5);
    let start = Instant::now();

    let mut handles = Vec::new();
    for client in clients.iter().cloned() {
        let handle = tokio::spawn(async move {
            for _ in 0..CONCURRENT_REQS_PER_CLIENT {
                let (reply, _) = client
                    .request_str("watchdog", &[], timeout)
                    .await
                    .unwrap();
                assert!(reply.is_ok());
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.unwrap();
    }
    let elapsed = start.elapsed();

    let total = CONCURRENT_CLIENTS as u64 * CONCURRENT_REQS_PER_CLIENT;
    println!();
    println!(
        "=== Concurrent Clients ({} clients x {} reqs) ===",
        CONCURRENT_CLIENTS, CONCURRENT_REQS_PER_CLIENT
    );
    println!(
        "  {} total requests in {:.3}s = {} aggregate",
        total,
        elapsed.as_secs_f64(),
        format_rate(total, elapsed)
    );
    println!(
        "  Per-client: {}",
        format_rate(CONCURRENT_REQS_PER_CLIENT, elapsed)
    );

    for c in &clients {
        c.disconnect().await;
    }
}

// ---------------------------------------------------------------------------
// 6. Sensor delivery: push fixed N, measure delivery
// ---------------------------------------------------------------------------

/// Benchmark sensor delivery speed.
#[tokio::test]
async fn bench_06_sensor_delivery() {
    let (server, port, _shutdown) = start_bench_server(1).await;
    let client = connect_client(port).await;
    let timeout = Duration::from_secs(5);

    let received = Arc::new(AtomicU64::new(0));
    let received_clone = received.clone();

    let reply = client
        .set_sensor_sampling("sensor-0", "event", &[], timeout, move |_update| {
            received_clone.fetch_add(1, Ordering::Relaxed);
        })
        .await
        .unwrap();
    assert!(reply.is_ok());

    // Let subscription settle
    tokio::time::sleep(Duration::from_millis(100)).await;
    received.store(0, Ordering::SeqCst);

    let sensors = server.context().sensors.read().await;
    let sensor = sensors.get("sensor-0").unwrap().clone();
    drop(sensors);

    // Push exactly DELIVERY_UPDATES as fast as possible
    let start = Instant::now();
    for i in 0..DELIVERY_UPDATES {
        sensor.set_int(i as i64, SensorStatus::Nominal);
    }
    let push_elapsed = start.elapsed();

    // Wait for drain
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let total_received = received.load(Ordering::Relaxed);

    println!();
    println!(
        "=== Sensor Delivery ({} updates, 1 sensor) ===",
        DELIVERY_UPDATES
    );
    println!(
        "  Push: {:.3}s = {}",
        push_elapsed.as_secs_f64(),
        format_rate(DELIVERY_UPDATES, push_elapsed)
    );
    println!(
        "  Delivered: {} / {} ({:.1}%)",
        total_received,
        DELIVERY_UPDATES,
        total_received as f64 / DELIVERY_UPDATES as f64 * 100.0
    );

    client.disconnect().await;
}

// ---------------------------------------------------------------------------
// 7. Multi-sensor delivery
// ---------------------------------------------------------------------------

/// Benchmark multiple sensors delivery speed.
#[tokio::test]
async fn bench_07_multi_sensor_delivery() {
    let (server, port, _shutdown) = start_bench_server(MULTI_SENSOR_COUNT).await;
    let client = connect_client(port).await;
    let timeout = Duration::from_secs(5);

    let received = Arc::new(AtomicU64::new(0));

    for i in 0..MULTI_SENSOR_COUNT {
        let counter = received.clone();
        let reply = client
            .set_sensor_sampling(
                &format!("sensor-{}", i),
                "event",
                &[],
                timeout,
                move |_update| {
                    counter.fetch_add(1, Ordering::Relaxed);
                },
            )
            .await
            .unwrap();
        assert!(reply.is_ok());
    }

    tokio::time::sleep(Duration::from_millis(100)).await;
    received.store(0, Ordering::SeqCst);

    let sensors: Vec<_> = {
        let sensor_map = server.context().sensors.read().await;
        (0..MULTI_SENSOR_COUNT)
            .map(|i| sensor_map.get(&format!("sensor-{}", i)).unwrap().clone())
            .collect()
    };

    let total_updates = MULTI_SENSOR_UPDATES_PER * MULTI_SENSOR_COUNT as u64;
    let start = Instant::now();
    for round in 0..MULTI_SENSOR_UPDATES_PER {
        for (i, sensor) in sensors.iter().enumerate() {
            sensor.set_int(
                (round * MULTI_SENSOR_COUNT as u64 + i as u64) as i64,
                SensorStatus::Nominal,
            );
        }
    }
    let push_elapsed = start.elapsed();

    tokio::time::sleep(Duration::from_millis(2000)).await;
    let total_received = received.load(Ordering::Relaxed);

    println!();
    println!(
        "=== Multi-Sensor Delivery ({} sensors x {} = {} updates) ===",
        MULTI_SENSOR_COUNT, MULTI_SENSOR_UPDATES_PER, total_updates
    );
    println!(
        "  Push: {:.3}s = {}",
        push_elapsed.as_secs_f64(),
        format_rate(total_updates, push_elapsed)
    );
    println!(
        "  Delivered: {} / {} ({:.1}%)",
        total_received,
        total_updates,
        total_received as f64 / total_updates as f64 * 100.0
    );

    client.disconnect().await;
}

// ---------------------------------------------------------------------------
// 8. High sensor count operations
// ---------------------------------------------------------------------------

/// Benchmark operations with a high number of sensors.
#[tokio::test]
async fn bench_08_high_sensor_count() {
    let (server, port, _shutdown) = start_bench_server(HIGH_SENSOR_COUNT).await;
    let client = connect_client(port).await;
    let timeout = Duration::from_secs(30);

    {
        let sensors = server.context().sensors.read().await;
        for i in 0..HIGH_SENSOR_COUNT {
            sensors
                .get(&format!("sensor-{}", i))
                .unwrap()
                .set_int(i as i64, SensorStatus::Nominal);
        }
    }

    // sensor-list all
    let start = Instant::now();
    for _ in 0..HIGH_SENSOR_LIST_ITERS {
        let (reply, informs) = client
            .request_str("sensor-list", &[], timeout)
            .await
            .unwrap();
        assert!(reply.is_ok());
        assert_eq!(informs.len(), HIGH_SENSOR_COUNT);
    }
    let elapsed = start.elapsed();

    println!();
    println!("=== High Sensor Count ({} sensors) ===", HIGH_SENSOR_COUNT);
    println!(
        "  sensor-list (all): {} reqs in {:.3}s = {}",
        HIGH_SENSOR_LIST_ITERS,
        elapsed.as_secs_f64(),
        format_rate(HIGH_SENSOR_LIST_ITERS, elapsed)
    );

    // sensor-value all
    let start = Instant::now();
    for _ in 0..HIGH_SENSOR_VALUE_ALL_ITERS {
        let (reply, informs) = client
            .request_str("sensor-value", &[], timeout)
            .await
            .unwrap();
        assert!(reply.is_ok());
        assert_eq!(informs.len(), HIGH_SENSOR_COUNT);
    }
    let elapsed = start.elapsed();

    println!(
        "  sensor-value (all): {} reqs in {:.3}s = {}",
        HIGH_SENSOR_VALUE_ALL_ITERS,
        elapsed.as_secs_f64(),
        format_rate(HIGH_SENSOR_VALUE_ALL_ITERS, elapsed)
    );

    // sensor-value single
    let start = Instant::now();
    for _ in 0..HIGH_SENSOR_VALUE_SINGLE_ITERS {
        let (reply, _) = client
            .request_str("sensor-value", &["sensor-500"], timeout)
            .await
            .unwrap();
        assert!(reply.is_ok());
    }
    let elapsed = start.elapsed();

    println!(
        "  sensor-value (single): {} reqs in {:.3}s = {}",
        HIGH_SENSOR_VALUE_SINGLE_ITERS,
        elapsed.as_secs_f64(),
        format_rate(HIGH_SENSOR_VALUE_SINGLE_ITERS, elapsed)
    );

    // sensor-list regex
    let start = Instant::now();
    for _ in 0..HIGH_SENSOR_REGEX_ITERS {
        let (reply, informs) = client
            .request_str("sensor-list", &["/sensor-1.*/"], timeout)
            .await
            .unwrap();
        assert!(reply.is_ok());
        assert!(!informs.is_empty());
    }
    let elapsed = start.elapsed();

    println!(
        "  sensor-list (regex): {} reqs in {:.3}s = {}",
        HIGH_SENSOR_REGEX_ITERS,
        elapsed.as_secs_f64(),
        format_rate(HIGH_SENSOR_REGEX_ITERS, elapsed)
    );

    client.disconnect().await;
}

// ---------------------------------------------------------------------------
// 9. Scale: 20K sensors
// ---------------------------------------------------------------------------

/// Benchmark scaling behavior with 20K sensors.
#[tokio::test]
async fn bench_09_scale_20k_sensors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let server = Arc::new(DeviceServer::new(
        "127.0.0.1",
        port,
        ("bench-server", 1, 0),
        ("bench-build", 1, 0, "bench"),
    ));

    let reg_start = Instant::now();
    for i in 0..SCALE_SENSOR_COUNT {
        server
            .add_sensor(Sensor::integer(
                &format!("sensor-{}", i),
                "bench sensor",
                "count",
                None,
                None,
                0,
            ))
            .await;
    }
    let reg_elapsed = reg_start.elapsed();

    let (_, shutdown_tx) = server.clone().start();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let client = connect_client(port).await;
    let timeout = Duration::from_secs(30);

    let received = Arc::new(AtomicU64::new(0));

    // Subscribe to all
    let sub_start = Instant::now();
    for i in 0..SCALE_SENSOR_COUNT {
        let counter = received.clone();
        let reply = client
            .set_sensor_sampling(
                &format!("sensor-{}", i),
                "event",
                &[],
                timeout,
                move |_update| {
                    counter.fetch_add(1, Ordering::Relaxed);
                },
            )
            .await
            .unwrap();
        assert!(reply.is_ok());
    }
    let sub_elapsed = sub_start.elapsed();

    // Reset counter after subscribe phase
    tokio::time::sleep(Duration::from_millis(200)).await;
    received.store(0, Ordering::SeqCst);

    let sensors: Vec<_> = {
        let sensor_map = server.context().sensors.read().await;
        (0..SCALE_SENSOR_COUNT)
            .map(|i| sensor_map.get(&format!("sensor-{}", i)).unwrap().clone())
            .collect()
    };

    // Push
    let total_pushed = SCALE_ROUNDS * SCALE_SENSOR_COUNT as u64;
    let push_start = Instant::now();
    for round in 0..SCALE_ROUNDS {
        for (i, sensor) in sensors.iter().enumerate() {
            sensor.set_int(
                (round * SCALE_SENSOR_COUNT as u64 + i as u64) as i64,
                SensorStatus::Nominal,
            );
        }
    }
    let push_elapsed = push_start.elapsed();

    tokio::time::sleep(Duration::from_millis(3000)).await;
    let total_received = received.load(Ordering::Relaxed);

    // Bulk queries
    let list_start = Instant::now();
    let (reply, informs) = client
        .request_str("sensor-list", &[], timeout)
        .await
        .unwrap();
    let list_elapsed = list_start.elapsed();
    assert!(reply.is_ok());
    assert_eq!(informs.len(), SCALE_SENSOR_COUNT);

    let value_start = Instant::now();
    let (reply, informs) = client
        .request_str("sensor-value", &[], timeout)
        .await
        .unwrap();
    let value_elapsed = value_start.elapsed();
    assert!(reply.is_ok());
    assert_eq!(informs.len(), SCALE_SENSOR_COUNT);

    println!();
    println!("=== Scale: {} Sensors ===", SCALE_SENSOR_COUNT);
    println!("  Registration: {:.3}s", reg_elapsed.as_secs_f64());
    println!(
        "  Subscribe all: {:.3}s = {}",
        sub_elapsed.as_secs_f64(),
        format_rate(SCALE_SENSOR_COUNT as u64, sub_elapsed)
    );
    println!(
        "  Push {} updates: {:.3}s = {}",
        total_pushed,
        push_elapsed.as_secs_f64(),
        format_rate(total_pushed, push_elapsed)
    );
    println!(
        "  Delivered: {} / {} ({:.1}%)",
        total_received,
        total_pushed,
        total_received as f64 / total_pushed as f64 * 100.0
    );
    println!("  sensor-list (all): {:.3}s", list_elapsed.as_secs_f64());
    println!(
        "  sensor-value (all): {:.3}s",
        value_elapsed.as_secs_f64()
    );

    client.disconnect().await;
    let _ = shutdown_tx.send(()).await;
}
