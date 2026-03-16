use std::time::Duration;

use katcp::client::DeviceClient;

/// Run the device client example.
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let host = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1".into());
    let port: u16 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(7147);

    println!("Connecting to KATCP server at {}:{}", host, port);

    let client = DeviceClient::new(&host, port);

    // Connect
    if let Err(e) = client.connect().await {
        eprintln!("Failed to connect: {}", e);
        return;
    }

    // Wait for protocol version
    match client.wait_protocol(Duration::from_secs(5)).await {
        Ok(pf) => println!("Connected! Protocol: {}", pf),
        Err(e) => {
            eprintln!("Failed to get protocol version: {}", e);
            return;
        }
    }

    let timeout = Duration::from_secs(5);

    // ?help - list all available requests
    println!("\n--- ?help ---");
    match client.request_str("help", &[], timeout).await {
        Ok((reply, informs)) => {
            for inform in &informs {
                println!("  {}", inform);
            }
            println!("Reply: {}", reply);
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    // ?sensor-list - list all sensors
    println!("\n--- ?sensor-list ---");
    match client.request_str("sensor-list", &[], timeout).await {
        Ok((reply, informs)) => {
            for inform in &informs {
                println!("  {}", inform);
            }
            println!("Reply: {}", reply);
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    // ?sensor-value - get a specific sensor value
    println!("\n--- ?sensor-value temperature ---");
    match client
        .request_str("sensor-value", &["temperature"], timeout)
        .await
    {
        Ok((reply, informs)) => {
            for inform in &informs {
                println!("  {}", inform);
            }
            println!("Reply: {}", reply);
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    // ?watchdog - ping
    println!("\n--- ?watchdog ---");
    match client.request_str("watchdog", &[], timeout).await {
        Ok((reply, _)) => println!("Reply: {}", reply),
        Err(e) => eprintln!("Error: {}", e),
    }

    // ?add 3.5 4.2 - custom request
    println!("\n--- ?add 3.5 4.2 ---");
    match client.request_str("add", &["3.5", "4.2"], timeout).await {
        Ok((reply, _)) => println!("Reply: {}", reply),
        Err(e) => eprintln!("Error: {}", e),
    }

    // ?echo hello world - custom request
    println!("\n--- ?echo hello world ---");
    match client
        .request_str("echo", &["hello", "world"], timeout)
        .await
    {
        Ok((reply, _)) => println!("Reply: {}", reply),
        Err(e) => eprintln!("Error: {}", e),
    }

    // ?sensor-sampling temperature event - subscribe with a callback
    println!("\n--- ?sensor-sampling temperature event ---");
    match client
        .set_sensor_sampling("temperature", "event", &[], timeout, |update| {
            println!(
                "  [sensor update] {} = {} (status: {})",
                update.name,
                update.value_str().unwrap_or("<binary>"),
                update.status.as_str(),
            );
        })
        .await
    {
        Ok(reply) => println!("Reply: {}", reply),
        Err(e) => eprintln!("Error: {}", e),
    }

    // Wait a bit and receive some sensor updates via the callback
    println!("\nWaiting for sensor updates (3 seconds)...");
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Clear sampling
    println!("\n--- clear sensor sampling ---");
    match client.clear_sensor_sampling("temperature", timeout).await {
        Ok(reply) => println!("Reply: {}", reply),
        Err(e) => eprintln!("Error: {}", e),
    }

    // ?version-list
    println!("\n--- ?version-list ---");
    match client.request_str("version-list", &[], timeout).await {
        Ok((reply, informs)) => {
            for inform in &informs {
                println!("  {}", inform);
            }
            println!("Reply: {}", reply);
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    // ?client-list
    println!("\n--- ?client-list ---");
    match client.request_str("client-list", &[], timeout).await {
        Ok((reply, informs)) => {
            for inform in &informs {
                println!("  {}", inform);
            }
            println!("Reply: {}", reply);
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    println!("\nDone!");
    client.disconnect().await;
}
