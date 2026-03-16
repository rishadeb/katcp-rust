# KATCP-Rust API Documentation

The `katcp-rust` crate provides a high-performance, asynchronous Rust implementation of the KATCP (Karoo Array Telescope Control Protocol) version 5. It includes both client and server implementations with full support for sensors, message routing, and sampling strategies.

## Core Message Protocol

### `Message`
The fundamental unit of KATCP communication.
*   **`Message::request(name: &str, args: Vec<Vec<u8>>) -> Self`**: Create a new request.
*   **`Message::reply(name: &str, args: Vec<Vec<u8>>) -> Self`**: Create a new reply.
*   **`Message::inform(name: &str, args: Vec<Vec<u8>>) -> Self`**: Create a new inform message.
*   **`to_bytes(&self) -> Vec<u8>`**: Serialize the message to the wire format.
*   **`arg_str(&self, index: usize) -> Option<&str>`**: Helper to safely decode a UTF-8 argument.

### `MessageType`
Enum representing the type of message (`Request`, `Reply`, `Inform`).

### `ProtocolFlags`
Handles KATCP protocol negotiation and feature flags (e.g., `MultiClient`, `MessageIds`).
*   **`ProtocolFlags::parse(version: &str) -> Option<Self>`**: Parses strings like `5.0-MI`.

---

## Server API

### `DeviceServer`
The main server structure. Manages client connections, routes requests, and handles sensor subscriptions.
*   **`DeviceServer::new(host: &str, port: u16, version_info: (&str, u32, u32), build_info: (&str, u32, u32, &str)) -> Self`**: Create a new server.
*   **`add_sensor(&self, sensor: Sensor)`**: Add a sensor to the server context.
*   **`register_request_handler<F>(&self, name: &str, handler: F)`**: Register a custom `?request` handler.
*   **`start(self: Arc<Self>) -> (JoinHandle<Result<()>>, mpsc::Sender<()>)`**: Run the server in a background Tokio task.

### `ServerContext`
Passed to custom request handlers. Provides thread-safe access to server state.
*   **`sensors: Arc<RwLock<HashMap<String, Sensor>>>`**: Global registry of sensors.
*   **`protocol: ProtocolFlags`**: The negotiated protocol version.

### `RequestHandler`
Signature for custom request callbacks:
`Arc<dyn Fn(&Message, &ServerContext) -> RequestResult + Send + Sync + 'static>`

---

## Client API

### `DeviceClient`
An asynchronous client for interfacing with KATCP servers.
*   **`DeviceClient::new(host: &str, port: u16) -> Arc<Self>`**: Initialize a client.
*   **`connect(&self) -> Result<()>`**: Establish the TCP connection and spawn reader/writer tasks.
*   **`request(&self, name: &str, args: Vec<Vec<u8>>, timeout: Duration) -> Result<(Message, Vec<Message>)>`**: Send a request and await the reply.
*   **`on_inform<F>(&self, name: &str, callback: F)`**: Register a callback for specific global informs.
*   **`set_sensor_sampling<F>(&self, sensor_name: &str, strategy: &str, params: &[&str], timeout: Duration, callback: F)`**: Subscribe to a sensor and handle its updates.
*   **`recv_inform(&self) -> Option<Message>`**: Await the next unsolicited inform message.

---

## Sensors and Sampling

### `Sensor`
A dynamic KATCP sensor with thread-safe value storage and observer notification.
*   **Factory methods**: `Sensor::integer`, `Sensor::float`, `Sensor::boolean`, `Sensor::discrete`, `Sensor::string`, `Sensor::timestamp`, `Sensor::lru`.
*   **`set_value(&self, value: Vec<u8>, status: SensorStatus, timestamp: Option<f64>)`**: Update the sensor. Helper methods like `set_int()`, `set_float()`, and `set_str()` provide convenient typed wrappers.
*   **`read(&self) -> SensorReading`**: Fetch the current reading.

### `SensorStatus`
Enum for sensor health: `Unknown`, `Nominal`, `Warn`, `Error`, `Failure`, `Unreachable`, `Inactive`.

### `SamplingStrategy`
Controls how often a sensor reports its value to subscribed clients. Supports: `None`, `Auto`, `Period`, `Event`, `Differential`, `EventRate`, `DifferentialRate`.
