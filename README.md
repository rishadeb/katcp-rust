# katcp-rust

[![CI](https://github.com/rishadeb/katcp-rust/actions/workflows/ci.yml/badge.svg)](https://github.com/rishadeb/katcp-rust/actions/workflows/ci.yml)

A Rust implementation of the [KATCP](https://katcp-python.readthedocs.io/) (Karoo Array Telescope Control Protocol) version 5, providing both a device server and async client library built on [tokio](https://tokio.rs/).

> **Disclaimer:** This library was developed entirely by Claude Opus (Anthropic) using the KATCP V5 protocol guidelines and [katcp-python](https://github.com/ska-sa/katcp-python) as the reference implementation. It has not been reviewed or endorsed by the SKA-SA team.

## Features

- **Device Server** — Full KATCP V5 server with built-in request handlers (`?watchdog`, `?help`, `?sensor-list`, `?sensor-value`, `?sensor-sampling`, `?version-list`, `?client-list`, `?log-level`, `?halt`)
- **Async Client** — Tokio-based client with automatic protocol negotiation, sensor sampling subscriptions, and callback-driven sensor updates
- **Sensor System** — Integer, float, boolean, discrete, string, timestamp, and LRU sensor types with per-client observer isolation
- **Sampling Strategies** — `none`, `auto`, `event`, `period`, `differential`, `event-rate`, `differential-rate`
- **Custom Request Handlers** — Register application-specific request handlers on the server
- **Message Protocol** — KATCP V5 message parsing/serialization with message ID support, escape handling, and type-safe encoding/decoding
- **Per-Client Isolation** — Sensor sampling subscriptions are tracked per-client; disconnecting one client does not affect another's subscriptions

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
katcp = { path = "." }
tokio = { version = "1", features = ["full"] }
```

### Server

```rust
use std::sync::Arc;
use katcp::server::DeviceServer;
use katcp::sensor::{Sensor, SensorStatus};

#[tokio::main]
async fn main() {
    let server = Arc::new(DeviceServer::new(
        "", 5000,
        ("my-device", 1, 0),
        ("my-build", 1, 0, "dev"),
    ));

    server.add_sensor(Sensor::float(
        "temperature", "Device temperature", "degC",
        Some(-40.0), Some(85.0), 25.0,
    )).await;

    let (handle, _shutdown) = server.start();
    handle.await.unwrap().unwrap();
}
```

### Client

```rust
use std::time::Duration;
use katcp::client::DeviceClient;

#[tokio::main]
async fn main() {
    let client = DeviceClient::new("127.0.0.1", 5000);
    client.connect().await.unwrap();
    client.wait_protocol(Duration::from_secs(2)).await.unwrap();

    // Subscribe to sensor updates
    client.set_sensor_sampling(
        "temperature", "event", &[],
        Duration::from_secs(2),
        |update| println!("{}: {} = {}", update.name, update.status.as_str(),
                          update.value_str().unwrap_or("?")),
    ).await.unwrap();
}
```

## Examples

```bash
# Run the sensor demo server
cargo run --example sensor_server

# In another terminal, connect with the client example or telnet
cargo run --example client
# or
telnet localhost 5001
```

## Building

```bash
# Build
cargo build

# Build (release)
cargo build --release

# Run tests
cargo test

# Run benchmarks (release mode recommended)
cargo test --release --test benchmarks

# Generate docs
cargo doc --open
```

## Benchmarks

Comparative results against [katcp-python](https://github.com/ska-sa/katcp-python) v0.9.3 and [aiokatcp](https://github.com/ska-sa/aiokatcp) v2.2.1 on Apple Silicon (loopback).

| Benchmark | katcp-rust | aiokatcp | katcp-python | Rust vs aiokatcp | Rust vs katcp-python |
|---|---|---|---|---|---|
| Watchdog latency | 18.6K req/s | 10.7K req/s | 4.7K req/s | 1.7x | 3.9x |
| Sensor value (single) | 14.3K req/s | 9.6K req/s | 3.6K req/s | 1.5x | 4.0x |
| Sensor value (10 sensors) | 11.3K req/s | 6.7K req/s | 1.6K req/s | 1.7x | 6.9x |
| 10 concurrent clients | 70.6K req/s | 24.5K req/s | 5.1K req/s | 2.9x | 14.0x |
| Sensor delivery (10K push) | 2.2M/s | 123K/s | 33.5K/s | 18.1x | 66.6x |
| 20K sensors subscribe | 36.9K/s | 7.2K/s | 2.8K/s | 5.1x | 13.1x |
| 20K sensors push (60K updates) | 2.0M/s | 137.7K/s | 36.1K/s | 14.3x | 54.6x |

Full results with latency percentiles: [BENCHMARKS.md](BENCHMARKS.md)

## Project Structure

```
src/
  lib.rs          Public API re-exports
  message.rs      KATCP message parsing and serialization
  types.rs        Type-safe KATCP type encoding/decoding
  protocol.rs     Protocol version flags
  sensor.rs       Sensor types, readings, and observer system
  sampling.rs     Sampling strategies and observer implementation
  server.rs       Device server with built-in request handlers
  client.rs       Async client with sensor subscription support
examples/
  device_server.rs    Basic server with custom handler
  sensor_server.rs    Server with multiple sensor types and simulated updates
  client.rs           Client connecting and querying a server
tests/
  integration.rs      End-to-end client/server integration tests
  benchmarks.rs       Performance benchmarks
```

## License

MIT
