# Katcp-Rust Server Tutorial

This tutorial walks through creating an asynchronous KATCP server using `DeviceServer` from scratch.

## Prerequisites & Setup

1. Create a new Rust project:
   ```bash
   cargo new my_katcp_server
   cd my_katcp_server
   ```

2. Add `katcp-rust` and `tokio` to your `Cargo.toml`:
   ```toml
   [dependencies]
   katcp-rust = { path = "path/to/katcp-rust" } # Or fetch from crates.io when published
   tokio = { version = "1.0", features = ["full"] }
   ```

3. Open `src/main.rs` and replace its contents with the **Full Server Example** code below.

4. Build and run your new server project:
   ```bash
   cargo build
   cargo run
   ```

## Full Server Example

The following code demonstrates how to initialize the server, add sensors, register custom command handlers using both named functions and anonymous closures, and start the listener all in one file.

```rust
use std::sync::Arc;
use katcp_rust::server::{DeviceServer, ServerContext, RequestResult};
use katcp_rust::message::Message;
use katcp_rust::sensor::{Sensor, SensorStatus};

// --- Named callback functions ---
//
// Any function with the signature:
//     fn(&Message, &ServerContext) -> RequestResult
// can be registered as a request handler.

/// Handler for ?echo <text> — echoes arguments back to the client.
fn handle_echo(msg: &Message, _ctx: &ServerContext) -> RequestResult {
    if let Some(text) = msg.arg_str(0) {
        RequestResult {
            informs: vec![],
            reply: Message::reply_to_request(msg, vec![b"ok".to_vec(), text.as_bytes().to_vec()]),
        }
    } else {
        RequestResult {
            informs: vec![],
            reply: Message::reply_to_request(msg, vec![b"fail".to_vec(), b"Missing argument".to_vec()]),
        }
    }
}

/// Handler for ?count — reads a sensor value via ServerContext.
fn handle_count(msg: &Message, ctx: &ServerContext) -> RequestResult {
    // Access the server's sensor registry through the context
    let value = ctx
        .sensors
        .try_read()
        .ok()
        .and_then(|sensors| sensors.get("packet-count").map(|s| s.read().value))
        .unwrap_or_else(|| b"unknown".to_vec());
    RequestResult {
        informs: vec![],
        reply: Message::reply_to_request(msg, vec![b"ok".to_vec(), value]),
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    // 1. Initializing the Server
    let server = Arc::new(DeviceServer::new(
        "0.0.0.0",
        7147,
        ("my-device", 1, 0),
        ("my-build-id", 1, 0, "rc1"),
    ));

    // 2. Adding Sensors
    let temp_sensor = Sensor::integer(
        "device.temperature",
        "The core temperature of the device",
        "degrees_C",
        Some(0),
        Some(100),
        25,
    );
    server.add_sensor(temp_sensor.clone()).await;

    // Simulate temperature fluctuations in a background task
    tokio::spawn(async move {
        let mut temp = 25;
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            temp += 1;
            let status = if temp > 80 { SensorStatus::Warn } else { SensorStatus::Nominal };
            temp_sensor.set_int(temp, status);
        }
    });

    // 3. Custom Request Handlers
    //
    // Option A: Named callback functions
    // Pass a function pointer directly — the cleanest approach for
    // non-trivial handlers or handlers you want to unit-test independently.
    server.register_request_handler("echo", handle_echo).await;
    server.register_request_handler("count", handle_count).await;

    // Option B: Anonymous closures
    // Convenient for simple, self-contained handlers.
    server
        .register_request_handler("greet", |msg, _ctx| {
            let name = msg.arg_str(0).unwrap_or("world");
            RequestResult {
                informs: vec![],
                reply: Message::reply_to_request(
                    msg,
                    vec![b"ok".to_vec(), format!("Hello, {}!", name).into_bytes()],
                ),
            }
        })
        .await;

    // 4. Starting the Server
    let (server_handle, shutdown_tx) = server.start();

    println!("Server is running at 0.0.0.0:7147. Press Ctrl+C to exit.");
    let _ = server_handle.await?;

    Ok(())
}
```

## Request Handler Signature

Both named functions and closures must match the handler signature:

```rust
Fn(&Message, &ServerContext) -> RequestResult + Send + Sync + 'static
```

- `msg: &Message` — the incoming request. Use `msg.arg_str(n)` to read arguments.
- `ctx: &ServerContext` — access to server state including the sensor registry (`ctx.sensors`), registered handlers, and version info.
- Returns `RequestResult` containing the reply `Message` and any inform messages.

### Named functions vs closures

| Approach | When to use |
|---|---|
| Named function (`fn handle_foo(msg, ctx) -> RequestResult`) | Non-trivial logic, reusable across servers, independently testable |
| Closure (`\|msg, _ctx\| { ... }`) | Simple one-liners, handlers that capture local state |

### Accessing sensors from a handler

Use `ServerContext` to read sensor values:

```rust
fn handle_status(msg: &Message, ctx: &ServerContext) -> RequestResult {
    let value = ctx
        .sensors
        .try_read()                    // non-blocking read lock
        .ok()
        .and_then(|sensors| {
            sensors.get("my-sensor")   // look up by name
                .map(|s| s.read().value) // read current value (Vec<u8>)
        })
        .unwrap_or_else(|| b"unknown".to_vec());

    RequestResult {
        informs: vec![],
        reply: Message::reply_to_request(msg, vec![b"ok".to_vec(), value]),
    }
}
```

### Returning inform messages

Handlers can return inform messages before the reply (e.g. for list-style responses):

```rust
fn handle_list_items(msg: &Message, _ctx: &ServerContext) -> RequestResult {
    let items = vec!["alpha", "beta", "gamma"];
    let informs: Vec<Message> = items
        .iter()
        .map(|item| Message::inform(&msg.name, vec![item.as_bytes().to_vec()]))
        .collect();
    RequestResult {
        informs,
        reply: Message::reply_to_request(
            msg,
            vec![b"ok".to_vec(), format!("{}", items.len()).into_bytes()],
        ),
    }
}
```

## Running the Repository Examples

If you cloned the `katcp-rust` repository, there are two complete server examples in the `examples/` directory: `device_server.rs` and `sensor_server.rs`.

To build and run them from the repository root:

```bash
# 1. Build all examples
cargo build --examples

# 2. Run a basic device server on port 7147
cargo run --example device_server

# OR, run the full sensor showcase server
cargo run --example sensor_server
```

Once the server is running, you can connect to it using standard terminal tools like `nc` (netcat) or `telnet` in a different terminal window:

```bash
nc 127.0.0.1 7147
```
