use std::sync::Arc;

use katcp::message::Message;
use katcp::sensor::{Sensor, SensorStatus};
use katcp::server::{DeviceServer, RequestResult, ServerContext};

// --- Named callback functions ---

/// Handler for ?echo <message> — echoes arguments back to the client.
fn handle_echo(msg: &Message, _ctx: &ServerContext) -> RequestResult {
    let echo_args = msg.arguments[..].to_vec();
    let mut reply_args = vec![b"ok".to_vec()];
    reply_args.extend(echo_args);
    RequestResult {
        informs: vec![],
        reply: Message::reply_to_request(msg, reply_args),
    }
}

/// Handler for ?add <x> <y> — adds two numbers and returns the result.
fn handle_add(msg: &Message, _ctx: &ServerContext) -> RequestResult {
    let x: f64 = msg
        .arg_str(0)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let y: f64 = msg
        .arg_str(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    let result = x + y;
    RequestResult {
        informs: vec![],
        reply: Message::reply_to_request(
            msg,
            vec![b"ok".to_vec(), format!("{}", result).into_bytes()],
        ),
    }
}

/// Handler for ?count — reads the packet-count sensor via ServerContext.
fn handle_count(msg: &Message, ctx: &ServerContext) -> RequestResult {
    let value = ctx
        .sensors
        .try_read()
        .ok()
        .and_then(|sensors| sensors.get("packet-count").map(|s| s.read().value))
        .unwrap_or_else(|| b"unknown".to_vec());
    RequestResult {
        informs: vec![],
        reply: Message::reply_to_request(
            msg,
            vec![b"ok".to_vec(), value],
        ),
    }
}

/// Run the device server example.
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let server = Arc::new(DeviceServer::new(
        "",    // bind to all interfaces
        7147,  // port (default KATCP port)
        ("example-device", 1, 0),
        ("example-build", 1, 0, "dev"),
    ));

    // Add sensors
    server
        .add_sensor(Sensor::float(
            "temperature",
            "Device temperature",
            "degC",
            Some(-40.0),
            Some(85.0),
            25.0,
        ))
        .await;

    server
        .add_sensor(Sensor::integer(
            "packet-count",
            "Number of packets processed",
            "count",
            Some(0),
            None,
            0,
        ))
        .await;

    server
        .add_sensor(Sensor::discrete(
            "device-status",
            "Overall device status",
            "",
            vec!["ok".into(), "warn".into(), "fail".into()],
            "ok",
        ))
        .await;

    server
        .add_sensor(Sensor::boolean(
            "power-on",
            "Whether device is powered on",
            "",
            false,
        ))
        .await;

    // Register request handlers using named callback functions
    server.register_request_handler("echo", handle_echo).await;
    server.register_request_handler("add", handle_add).await;
    server.register_request_handler("count", handle_count).await;

    // Anonymous closures also work — useful for simple one-off handlers
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

    // Bind first so we fail fast if port is in use
    let (listener, addr) = match server.bind().await {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to bind to port 7147: {}", e);
            std::process::exit(1);
        }
    };

    println!("KATCP device server listening on {}", addr);
    println!("Connect with: telnet localhost {}", addr.port());
    println!("Try: ?help, ?sensor-list, ?sensor-value temperature");
    println!("     ?echo hello, ?add 3 4, ?count, ?greet Alice");

    // Start a background task to update sensors periodically
    let server_ref = server.clone();
    tokio::spawn(async move {
        let mut count = 0u64;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let sensors = server_ref.context().sensors.read().await;

            if let Some(temp) = sensors.get("temperature") {
                let temp_val = 25.0 + (count as f64 * 0.1).sin() * 5.0;
                temp.set_float(temp_val, SensorStatus::Nominal);
            }
            if let Some(pkt) = sensors.get("packet-count") {
                count += 1;
                pkt.set_int(count as i64, SensorStatus::Nominal);
            }
            if let Some(power) = sensors.get("power-on") {
                power.set_bool(true, SensorStatus::Nominal);
            }
        }
    });

    // Run the server with the pre-bound listener (Ctrl+C to stop)
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel(1);

    // Handle Ctrl+C gracefully
    let shutdown = shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        println!("\nShutting down...");
        let _ = shutdown.send(()).await;
    });

    if let Err(e) = server.run_with_listener(listener, &mut shutdown_rx).await {
        eprintln!("Server error: {}", e);
    }
}
