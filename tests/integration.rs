use std::sync::Arc;
use std::time::Duration;

use katcp::client::DeviceClient;
use katcp::message::Message;
use katcp::sensor::{Sensor, SensorStatus};
use katcp::server::{DeviceServer, RequestResult};

/// Helper to start a test server on a random port with sensors and custom handlers.
/// Uses a two-step approach: bind a listener to get a port, then start the server on that port.
async fn start_server() -> (Arc<DeviceServer>, u16, tokio::sync::mpsc::Sender<()>) {
    // Find a free port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let server = Arc::new(DeviceServer::new(
        "127.0.0.1",
        port,
        ("integration-test", 1, 0),
        ("test-build", 1, 0, "dev"),
    ));

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
            "counter",
            "A counter",
            "count",
            Some(0),
            None,
            0,
        ))
        .await;

    server
        .add_sensor(Sensor::boolean("flag", "A flag", "", false))
        .await;

    server
        .add_sensor(Sensor::discrete(
            "mode",
            "Operating mode",
            "",
            vec!["idle".into(), "active".into(), "error".into()],
            "idle",
        ))
        .await;

    // Custom handler: ?multiply <x> <y>
    server
        .register_request_handler("multiply", |msg, _ctx| {
            let x: f64 = msg
                .arg_str(0)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            let y: f64 = msg
                .arg_str(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            RequestResult {
                informs: vec![],
                reply: Message::reply_to_request(
                    msg,
                    vec![b"ok".to_vec(), format!("{}", x * y).into_bytes()],
                ),
            }
        })
        .await;

    let (_, shutdown_tx) = server.clone().start();

    // Give the server a moment to start listening
    tokio::time::sleep(Duration::from_millis(50)).await;

    (server, port, shutdown_tx)
}

/// Test the full flow of client and server communication.
#[tokio::test]
async fn test_full_client_server_flow() {
    let (server, port, _shutdown) = start_server().await;

    let client = DeviceClient::new("127.0.0.1", port);
    client.connect().await.unwrap();

    let pf = client.wait_protocol(Duration::from_secs(2)).await.unwrap();
    assert_eq!(pf.major, 5);

    let timeout = Duration::from_secs(2);

    // Watchdog
    let (reply, _) = client.request_str("watchdog", &[], timeout).await.unwrap();
    assert!(reply.is_ok());

    // Help
    let (reply, informs) = client.request_str("help", &[], timeout).await.unwrap();
    assert!(reply.is_ok());
    assert!(informs.len() >= 9); // built-in + custom

    // Sensor list
    let (reply, informs) = client
        .request_str("sensor-list", &[], timeout)
        .await
        .unwrap();
    assert!(reply.is_ok());
    assert_eq!(informs.len(), 4); // temperature, counter, flag, mode

    // Set sensor value and read it
    {
        let sensors = server.context().sensors.read().await;
        sensors
            .get("temperature")
            .unwrap()
            .set_float(37.5, SensorStatus::Nominal);
    }

    let (reply, informs) = client
        .request_str("sensor-value", &["temperature"], timeout)
        .await
        .unwrap();
    assert!(reply.is_ok());
    assert_eq!(informs.len(), 1);
    assert_eq!(informs[0].arg_str(2), Some("temperature"));
    assert_eq!(informs[0].arg_str(3), Some("nominal"));

    // Custom handler
    let (reply, _) = client
        .request_str("multiply", &["3", "7"], timeout)
        .await
        .unwrap();
    assert!(reply.is_ok());
    assert_eq!(reply.arg_str(1), Some("21"));

    // Version list
    let (reply, informs) = client
        .request_str("version-list", &[], timeout)
        .await
        .unwrap();
    assert!(reply.is_ok());
    assert_eq!(informs.len(), 3);

    // Client list (should include ourselves)
    let (reply, informs) = client
        .request_str("client-list", &[], timeout)
        .await
        .unwrap();
    assert!(reply.is_ok());
    assert_eq!(informs.len(), 1);

    // Log level
    let (reply, _) = client
        .request_str("log-level", &["info"], timeout)
        .await
        .unwrap();
    assert!(reply.is_ok());

    client.disconnect().await;
}

/// Test handling multiple concurrent clients.
#[tokio::test]
async fn test_multiple_clients() {
    let (_server, port, _shutdown) = start_server().await;

    let client1 = DeviceClient::new("127.0.0.1", port);
    let client2 = DeviceClient::new("127.0.0.1", port);

    client1.connect().await.unwrap();
    client2.connect().await.unwrap();

    client1
        .wait_protocol(Duration::from_secs(2))
        .await
        .unwrap();
    client2
        .wait_protocol(Duration::from_secs(2))
        .await
        .unwrap();

    let timeout = Duration::from_secs(2);

    // Both clients should work independently
    let (r1, _) = client1
        .request_str("watchdog", &[], timeout)
        .await
        .unwrap();
    let (r2, _) = client2
        .request_str("watchdog", &[], timeout)
        .await
        .unwrap();
    assert!(r1.is_ok());
    assert!(r2.is_ok());

    // Client list should show 2 clients
    let (reply, informs) = client1
        .request_str("client-list", &[], timeout)
        .await
        .unwrap();
    assert!(reply.is_ok());
    assert_eq!(informs.len(), 2);

    client1.disconnect().await;
    client2.disconnect().await;
}

/// Test sensor sampling subscriptions and inform updates.
#[tokio::test]
async fn test_sensor_sampling() {
    let (server, port, _shutdown) = start_server().await;

    let client = DeviceClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    client
        .wait_protocol(Duration::from_secs(2))
        .await
        .unwrap();

    let timeout = Duration::from_secs(2);

    // Subscribe to counter sensor with event strategy and a callback
    let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
    let updates_clone = updates.clone();

    let reply = client
        .set_sensor_sampling("counter", "event", &[], timeout, move |update| {
            updates_clone.lock().unwrap().push(update.clone());
        })
        .await
        .unwrap();
    assert!(reply.is_ok());
    assert_eq!(reply.arg_str(1), Some("counter"));
    assert_eq!(reply.arg_str(2), Some("event"));

    // Update the sensor - should trigger a callback
    {
        let sensors = server.context().sensors.read().await;
        sensors
            .get("counter")
            .unwrap()
            .set_int(42, SensorStatus::Nominal);
    }

    // Give the inform a moment to arrive
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify the callback was invoked with the correct data
    {
        let received = updates.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].name, "counter");
        assert_eq!(received[0].status, SensorStatus::Nominal);
        assert_eq!(received[0].value_str(), Some("42"));
    }

    // Update again with a different value
    {
        let sensors = server.context().sensors.read().await;
        sensors
            .get("counter")
            .unwrap()
            .set_int(99, SensorStatus::Warn);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    {
        let received = updates.lock().unwrap();
        assert_eq!(received.len(), 2);
        assert_eq!(received[1].name, "counter");
        assert_eq!(received[1].status, SensorStatus::Warn);
        assert_eq!(received[1].value_str(), Some("99"));
    }

    // Clear sampling for this sensor
    let reply = client.clear_sensor_sampling("counter", timeout).await.unwrap();
    assert!(reply.is_ok());

    // Update again — should NOT trigger a callback
    {
        let sensors = server.context().sensors.read().await;
        sensors
            .get("counter")
            .unwrap()
            .set_int(200, SensorStatus::Nominal);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    {
        let received = updates.lock().unwrap();
        assert_eq!(received.len(), 2); // still 2, no new update
    }

    client.disconnect().await;
}

/// Test that disconnecting one client does not affect another client's sampling.
#[tokio::test]
async fn test_disconnect_cleanup_isolates_clients() {
    let (server, port, _shutdown) = start_server().await;

    let client1 = DeviceClient::new("127.0.0.1", port);
    let client2 = DeviceClient::new("127.0.0.1", port);

    client1.connect().await.unwrap();
    client2.connect().await.unwrap();
    client1.wait_protocol(Duration::from_secs(2)).await.unwrap();
    client2.wait_protocol(Duration::from_secs(2)).await.unwrap();

    let timeout = Duration::from_secs(2);

    // Both clients subscribe to the same sensor
    let updates1 = Arc::new(std::sync::Mutex::new(Vec::new()));
    let updates2 = Arc::new(std::sync::Mutex::new(Vec::new()));

    let u1 = updates1.clone();
    client1
        .set_sensor_sampling("counter", "event", &[], timeout, move |u| {
            u1.lock().unwrap().push(u.clone());
        })
        .await
        .unwrap();

    let u2 = updates2.clone();
    client2
        .set_sensor_sampling("counter", "event", &[], timeout, move |u| {
            u2.lock().unwrap().push(u.clone());
        })
        .await
        .unwrap();

    // Update sensor — both clients should receive
    {
        let sensors = server.context().sensors.read().await;
        sensors.get("counter").unwrap().set_int(1, SensorStatus::Nominal);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(updates1.lock().unwrap().len(), 1);
    assert_eq!(updates2.lock().unwrap().len(), 1);

    // Disconnect client1 — its sampling should be cleaned up
    client1.disconnect().await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Update sensor again — only client2 should receive
    {
        let sensors = server.context().sensors.read().await;
        sensors.get("counter").unwrap().set_int(2, SensorStatus::Nominal);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(updates1.lock().unwrap().len(), 1); // unchanged, client1 disconnected
    assert_eq!(updates2.lock().unwrap().len(), 2); // client2 still receiving

    client2.disconnect().await;
}

/// Test fetching all sensor values.
#[tokio::test]
async fn test_sensor_value_all() {
    let (_server, port, _shutdown) = start_server().await;

    let client = DeviceClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    client
        .wait_protocol(Duration::from_secs(2))
        .await
        .unwrap();

    let timeout = Duration::from_secs(2);

    // No sensor name → return all sensor values
    let (reply, informs) = client
        .request_str("sensor-value", &[], timeout)
        .await
        .unwrap();
    assert!(reply.is_ok());
    assert_eq!(informs.len(), 4); // temperature, counter, flag, mode
}

/// Test fetching an unknown sensor value.
#[tokio::test]
async fn test_sensor_value_unknown() {
    let (_server, port, _shutdown) = start_server().await;

    let client = DeviceClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    client
        .wait_protocol(Duration::from_secs(2))
        .await
        .unwrap();

    let timeout = Duration::from_secs(2);

    let (reply, _) = client
        .request_str("sensor-value", &["nonexistent"], timeout)
        .await
        .unwrap();
    assert!(!reply.is_ok());
    assert_eq!(reply.reply_status(), Some("fail"));
    assert_eq!(reply.arg_str(1), Some("Unknown sensor name."));

    client.disconnect().await;
}

/// Test fetching help for a specific request.
#[tokio::test]
async fn test_help_specific_request() {
    let (_server, port, _shutdown) = start_server().await;

    let client = DeviceClient::new("127.0.0.1", port);
    client.connect().await.unwrap();
    client
        .wait_protocol(Duration::from_secs(2))
        .await
        .unwrap();

    let timeout = Duration::from_secs(2);

    // Help for known request
    let (reply, informs) = client
        .request_str("help", &["watchdog"], timeout)
        .await
        .unwrap();
    assert!(reply.is_ok());
    assert_eq!(informs.len(), 1);

    // Help for custom handler
    let (reply, informs) = client
        .request_str("help", &["multiply"], timeout)
        .await
        .unwrap();
    assert!(reply.is_ok());
    assert_eq!(informs.len(), 1);

    // Help for unknown request
    let (reply, _) = client
        .request_str("help", &["nonexistent"], timeout)
        .await
        .unwrap();
    assert!(!reply.is_ok());

    client.disconnect().await;
}
