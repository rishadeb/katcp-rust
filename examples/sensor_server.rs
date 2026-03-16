use std::sync::Arc;

use katcp::sensor::{Sensor, SensorStatus};
use katcp::server::DeviceServer;

/// Example: A server focused on demonstrating the sensor system with
/// multiple sensor types and periodic updates.
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let server = Arc::new(DeviceServer::new(
        "",
        5001,
        ("sensor-demo", 1, 0),
        ("sensor-demo-build", 1, 0, "dev"),
    ));

    // Integer sensor with range
    server
        .add_sensor(Sensor::integer(
            "adc-value",
            "Raw ADC reading",
            "counts",
            Some(0),
            Some(4095),
            0,
        ))
        .await;

    // Float sensor with range
    server
        .add_sensor(Sensor::float(
            "wind-speed",
            "Current wind speed",
            "m/s",
            Some(0.0),
            Some(100.0),
            0.0,
        ))
        .await;

    // Boolean sensor
    server
        .add_sensor(Sensor::boolean(
            "door-open",
            "Whether the equipment door is open",
            "",
            false,
        ))
        .await;

    // Discrete sensor
    server
        .add_sensor(Sensor::discrete(
            "receiver-band",
            "Currently selected receiver band",
            "",
            vec![
                "L-band".into(),
                "S-band".into(),
                "C-band".into(),
                "X-band".into(),
            ],
            "L-band",
        ))
        .await;

    // String sensor
    server
        .add_sensor(Sensor::string(
            "last-command",
            "Last command received",
            "",
            "",
        ))
        .await;

    // LRU sensor
    server
        .add_sensor(Sensor::lru(
            "digitiser-health",
            "Health status of the digitiser",
            "",
            true,
        ))
        .await;

    let (listener, addr) = match server.bind().await {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to bind to port 5001: {}", e);
            std::process::exit(1);
        }
    };

    println!("Sensor demo server listening on {}", addr);
    println!("Connect with: telnet localhost {}", addr.port());
    println!("Try: ?sensor-list, ?sensor-value wind-speed");
    println!("     ?sensor-sampling wind-speed event");

    // Simulate sensor updates
    let server_ref = server.clone();
    tokio::spawn(async move {
        let mut tick = 0u64;
        let bands = ["L-band", "S-band", "C-band", "X-band"];

        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            tick += 1;

            let sensors = server_ref.context().sensors.read().await;

            // ADC value: sawtooth wave
            if let Some(s) = sensors.get("adc-value") {
                let val = ((tick * 100) % 4096) as i64;
                s.set_int(val, SensorStatus::Nominal);
            }

            // Wind speed: sinusoidal
            if let Some(s) = sensors.get("wind-speed") {
                let val = 15.0 + 10.0 * (tick as f64 * 0.1).sin();
                let status = if val > 20.0 {
                    SensorStatus::Warn
                } else {
                    SensorStatus::Nominal
                };
                s.set_float(val, status);
            }

            // Door: toggle every 20 ticks
            if let Some(s) = sensors.get("door-open") {
                let open = (tick / 20) % 2 == 1;
                let status = if open {
                    SensorStatus::Warn
                } else {
                    SensorStatus::Nominal
                };
                s.set_bool(open, status);
            }

            // Receiver band: cycle every 30 ticks
            if let Some(s) = sensors.get("receiver-band") {
                let band = bands[((tick / 30) % 4) as usize];
                s.set_str(band, SensorStatus::Nominal);
            }

            // Digitiser health: occasional error
            if let Some(s) = sensors.get("digitiser-health") {
                let nominal = tick % 50 != 0;
                s.set_value(
                    if nominal {
                        b"nominal".to_vec()
                    } else {
                        b"error".to_vec()
                    },
                    if nominal {
                        SensorStatus::Nominal
                    } else {
                        SensorStatus::Error
                    },
                    None,
                );
            }
        }
    });

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel(1);
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
