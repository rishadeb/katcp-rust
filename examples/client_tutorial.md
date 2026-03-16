# Katcp-Rust Client Tutorial

This tutorial walks through creating an asynchronous KATCP client using `DeviceClient` from scratch.

## Prerequisites & Setup

1. Create a new Rust project:
   ```bash
   cargo new my_katcp_client
   cd my_katcp_client
   ```

2. Add `katcp-rust` and `tokio` (for the async runtime) to your `Cargo.toml`:
   ```toml
   [dependencies]
   katcp-rust = { path = "path/to/katcp-rust" } # Or fetch from crates.io when published
   tokio = { version = "1.0", features = ["full"] }
   ```

3. Open `src/main.rs` and replace its contents with the **Full Client Example** code below.

4. Build and run your new client project (make sure you have a KATCP server running first, see the server tutorial!):
   ```bash
   cargo build
   cargo run
   ```

## Full Client Example

The following code demonstrates a complete flow for connecting to a server, sending requests, subscribing to sensors, and handling unsolicited messages.

```rust
use std::sync::Arc;
use std::time::Duration;
use katcp_rust::client::DeviceClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialize the client
    let client = DeviceClient::new("127.0.0.1", 7147);
    
    // 2. Connect to the server. This spawns background reader/writer tasks.
    client.connect().await?;
    
    // 3. Wait for the server to send its #version-connect inform (protocol negotiation)
    let protocol = client.wait_protocol(Duration::from_secs(5)).await?;
    println!("Connected using KATCP protocol version: {}", protocol);
    
    // 4. Sending Requests
    // Send a ?help request
    let (reply, informs) = client
        .request_str("help", &[], Duration::from_secs(5))
        .await?;
        
    if reply.is_ok() {
        println!("Server responded with {} help items:", informs.len());
        for inform in informs {
            if let (Some(name), Some(desc)) = (inform.arg_str(0), inform.arg_str(1)) {
                println!("  ?{}: {}", name, desc);
            }
        }
    }

    // 5. Subscribing to Sensors
    // We want to subscribe to updates from "system.cpu.load" whenever it changes (strategy: "event")
    let timeout = Duration::from_secs(5);
    
    let reply = client.set_sensor_sampling(
        "system.cpu.load", 
        "event", 
        &[], // No additional parameters needed for "event" strategy
        timeout,
        |update| {
            // This callback is fired asynchronously whenever a #sensor-status inform arrives
            let val = update.value_str().unwrap_or("unknown");
            let status = update.status.as_str();
            println!("[{:.3}] CPU Load = {} ({})", update.timestamp, val, status);
        }
    ).await?;
    
    if reply.is_ok() {
        println!("Successfully subscribed to CPU load sensor.");
    }
    
    // To cleanly stop sampling later, you can use:
    // client.clear_sensor_sampling("system.cpu.load", timeout).await?;

    // 6. Unsolicited Informs
    // Listen for unsolicited messages (like #log) in a spawned task
    let client_clone = client.clone();
    tokio::spawn(async move {
        while let Some(msg) = client_clone.recv_inform().await {
            if msg.name == "log" {
                let level = msg.arg_str(0).unwrap_or("unknown");
                let log_msg = msg.arg_str(3).unwrap_or("");
                println!("SERVER LOG [{}]: {}", level, log_msg);
            }
        }
    });

    // 7. Prevent the main function from exiting immediately
    tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
    Ok(())
}
```

## Running the Repository Examples

If you cloned the `katcp-rust` repository, there is already a complete `client.rs` example in the `examples/` directory.

To build and run it against a locally running server on port 7147, run these commands from the `katcp-rust` root directory:

```bash
# 1. Build the examples
cargo build --examples

# 2. Run the client example, passing the host and port
cargo run --example client -- 127.0.0.1 7147
```
