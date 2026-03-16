use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

use crate::message::{Message, MessageParser, MessageType};
use crate::protocol::ProtocolFlags;
use crate::sensor::SensorStatus;

/// Callback type for inform messages.
pub type InformCallback = Arc<dyn Fn(&Message) + Send + Sync + 'static>;

/// Callback type for sensor sampling updates.
pub type SensorCallback = Arc<dyn Fn(&SensorUpdate) + Send + Sync + 'static>;

/// A parsed sensor status update received from the server.
#[derive(Debug, Clone)]
pub struct SensorUpdate {
    pub timestamp: f64,
    pub name: String,
    pub status: SensorStatus,
    pub value: Vec<u8>,
}

impl SensorUpdate {
    /// Get the value as a UTF-8 string.
    ///
    /// # Returns
    ///
    /// `Some(&str)` if the byte vector is valid UTF-8, `None` otherwise.
    pub fn value_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.value).ok()
    }

    /// Try to parse from a `#sensor-status` inform message.
    /// Format: #sensor-status <timestamp> <count> <name> <status> <value>
    fn from_inform(msg: &Message) -> Option<Self> {
        if msg.name != "sensor-status" || msg.arguments.len() < 5 {
            return None;
        }
        let timestamp: f64 = msg.arg_str(0)?.parse().ok()?;
        let name = msg.arg_str(2)?.to_string();
        let status = SensorStatus::from_str(msg.arg_str(3)?)?;
        let value = msg.arguments[4].clone();
        Some(Self {
            timestamp,
            name,
            status,
            value,
        })
    }
}

/// A pending request waiting for its reply.
struct PendingRequest {
    reply_tx: oneshot::Sender<(Message, Vec<Message>)>,
    informs: Vec<Message>,
}

/// A KATCP device client.
pub struct DeviceClient {
    host: String,
    port: u16,
    write_tx: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    pending: Arc<Mutex<HashMap<String, PendingRequest>>>,
    next_mid: Mutex<u64>,
    protocol: RwLock<Option<ProtocolFlags>>,
    inform_callbacks: RwLock<HashMap<String, InformCallback>>,
    sensor_callbacks: Arc<RwLock<HashMap<String, SensorCallback>>>,
    connected: RwLock<bool>,
    // Receiver for unsolicited informs (no mid)
    unsolicited_tx: mpsc::UnboundedSender<Message>,
    unsolicited_rx: Mutex<mpsc::UnboundedReceiver<Message>>,
}

impl DeviceClient {
    /// Create a new client targeting the given host and port.
    ///
    /// # Arguments
    ///
    /// * `host` - The hostname or IP address of the KATCP server.
    /// * `port` - The port to connect to.
    ///
    /// # Returns
    ///
    /// An unconnected `DeviceClient` wrapped in an `Arc`.
    pub fn new(host: &str, port: u16) -> Arc<Self> {
        let (unsolicited_tx, unsolicited_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            host: host.to_string(),
            port,
            write_tx: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_mid: Mutex::new(1),
            protocol: RwLock::new(None),
            inform_callbacks: RwLock::new(HashMap::new()),
            sensor_callbacks: Arc::new(RwLock::new(HashMap::new())),
            connected: RwLock::new(false),
            unsolicited_tx,
            unsolicited_rx: Mutex::new(unsolicited_rx),
        })
    }

    /// Register a callback for a specific inform message name.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the inform message to listen for (e.g., "log").
    /// * `callback` - The function to call when the inform is received.
    pub async fn on_inform<F>(&self, name: &str, callback: F)
    where
        F: Fn(&Message) + Send + Sync + 'static,
    {
        self.inform_callbacks
            .write()
            .await
            .insert(name.to_string(), Arc::new(callback));
    }

    /// Connect to the server and start the message handling loop.
    ///
    /// # Returns
    ///
    /// `Ok(())` upon successful connection.
    ///
    /// # Errors
    ///
    /// Returns an `std::io::Error` if establishing the TCP connection fails.
    pub async fn connect(self: &Arc<Self>) -> std::io::Result<()> {
        let addr = format!("{}:{}", self.host, self.port);
        let stream = TcpStream::connect(&addr).await?;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        // Channel for outgoing messages
        let (write_tx, mut write_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        *self.write_tx.lock().await = Some(write_tx);
        *self.connected.write().await = true;

        // Spawn writer task
        tokio::spawn(async move {
            while let Some(data) = write_rx.recv().await {
                if writer.write_all(&data).await.is_err() {
                    break;
                }
            }
        });

        // Spawn reader task
        let this = self.clone();

        tokio::spawn(async move {
            let mut line_buf = String::new();
            loop {
                line_buf.clear();
                match reader.read_line(&mut line_buf).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let line = line_buf.trim_end_matches('\n').trim_end_matches('\r');
                        if line.is_empty() {
                            continue;
                        }
                        match MessageParser::parse(line.as_bytes()) {
                            Ok(msg) => {
                                Self::dispatch_message(
                                    &msg,
                                    &this.pending,
                                    &this.protocol,
                                    &this.inform_callbacks,
                                    &this.sensor_callbacks,
                                    &this.unsolicited_tx,
                                )
                                .await;
                            }
                            Err(e) => {
                                tracing::warn!("Parse error: {}", e);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            *this.connected.write().await = false;
        });

        Ok(())
    }

    /// Dispatch an incoming message to the appropriate handlers or channels.
    async fn dispatch_message(
        msg: &Message,
        pending: &Arc<Mutex<HashMap<String, PendingRequest>>>,
        protocol: &RwLock<Option<ProtocolFlags>>,
        inform_callbacks: &RwLock<HashMap<String, InformCallback>>,
        sensor_callbacks: &Arc<RwLock<HashMap<String, SensorCallback>>>,
        unsolicited_tx: &mpsc::UnboundedSender<Message>,
    ) {
        match msg.mtype {
            MessageType::Inform => {
                // Check if this is a version-connect inform
                if msg.name == "version-connect" {
                    if msg.arg_str(0) == Some("katcp-protocol") {
                        if let Some(version_str) = msg.arg_str(1) {
                            if let Some(pf) = ProtocolFlags::parse(version_str) {
                                *protocol.write().await = Some(pf);
                            }
                        }
                    }
                }

                // If it has a mid, it's a reply-inform for a pending request
                if let Some(ref mid) = msg.mid {
                    let mut pending = pending.lock().await;
                    if let Some(req) = pending.get_mut(mid) {
                        req.informs.push(msg.clone());
                        return;
                    }
                }

                // Dispatch #sensor-status informs to per-sensor callbacks
                if msg.name == "sensor-status" {
                    if let Some(update) = SensorUpdate::from_inform(msg) {
                        let cbs = sensor_callbacks.read().await;
                        if let Some(cb) = cbs.get(&update.name) {
                            cb(&update);
                        }
                    }
                }

                // Check inform callbacks
                let callbacks = inform_callbacks.read().await;
                if let Some(cb) = callbacks.get(&msg.name) {
                    cb(msg);
                }

                // Send to unsolicited channel
                let _ = unsolicited_tx.send(msg.clone());
            }
            MessageType::Reply => {
                if let Some(ref mid) = msg.mid {
                    let mut pending = pending.lock().await;
                    if let Some(req) = pending.remove(mid) {
                        let _ = req.reply_tx.send((msg.clone(), req.informs));
                    }
                } else {
                    // Reply without mid - try to match by name
                    let mut pending = pending.lock().await;
                    // Find first pending request with matching name
                    let key = pending
                        .keys()
                        .next()
                        .cloned();
                    if let Some(key) = key {
                        if let Some(req) = pending.remove(&key) {
                            let _ = req.reply_tx.send((msg.clone(), req.informs));
                        }
                    }
                }
            }
            MessageType::Request => {
                // Clients don't typically receive requests, but log it
                tracing::debug!("Received unexpected request: {}", msg);
            }
        }
    }

    /// Send a raw message to the server.
    ///
    /// # Arguments
    ///
    /// * `msg` - The KATCP `Message` to send.
    ///
    /// # Returns
    ///
    /// `true` if the message was successfully queued for sending, `false` otherwise.
    pub async fn send_message(&self, msg: &Message) -> bool {
        if let Some(ref tx) = *self.write_tx.lock().await {
            tx.send(msg.to_bytes()).is_ok()
        } else {
            false
        }
    }

    /// Send a request and wait for the reply, with a timeout.
    /// Returns the reply message and any associated inform messages.
    ///
    /// # Arguments
    ///
    /// * `name` - The request name (e.g., "sensor-value").
    /// * `args` - A vector of raw bytes representing arguments to the request.
    /// * `timeout` - The maximum duration to wait for a reply.
    ///
    /// # Returns
    ///
    /// `Ok((reply_message, inform_messages))` on success.
    ///
    /// # Errors
    /// 
    /// Returns `ClientError::Timeout` if the server does not reply in time, or `ClientError::NotConnected`
    /// if the connection is lost.
    pub async fn request(
        &self,
        name: &str,
        args: Vec<Vec<u8>>,
        timeout: Duration,
    ) -> Result<(Message, Vec<Message>), ClientError> {
        let mid = {
            let mut next = self.next_mid.lock().await;
            let mid = next.to_string();
            *next += 1;
            mid
        };

        let msg = Message::request(name, args).with_mid(&mid);

        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(
                mid.clone(),
                PendingRequest {
                    reply_tx,
                    informs: Vec::new(),
                },
            );
        }

        if !self.send_message(&msg).await {
            self.pending.lock().await.remove(&mid);
            return Err(ClientError::NotConnected);
        }

        match tokio::time::timeout(timeout, reply_rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&mid);
                Err(ClientError::ChannelClosed)
            }
            Err(_) => {
                self.pending.lock().await.remove(&mid);
                Err(ClientError::Timeout)
            }
        }
    }

    /// Convenience method: send a request with string arguments.
    ///
    /// # Arguments
    ///
    /// * `name` - The request name.
    /// * `args` - A slice of string arguments.
    /// * `timeout` - The maximum duration to wait for a reply.
    ///
    /// # Returns
    ///
    /// `Ok((reply_message, inform_messages))` on success.
    ///
    /// # Errors
    ///
    /// Returns a `ClientError` if a timeout occurs or the connection drops.
    pub async fn request_str(
        &self,
        name: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<(Message, Vec<Message>), ClientError> {
        let args: Vec<Vec<u8>> = args.iter().map(|s| s.as_bytes().to_vec()).collect();
        self.request(name, args, timeout).await
    }

    /// Set sensor sampling on the server and register a callback for updates.
    ///
    /// Sends `?sensor-sampling <sensor_name> <strategy> [params...]` to the server.
    /// When the server sends `#sensor-status` informs for this sensor, the callback
    /// is invoked with a parsed `SensorUpdate`.
    ///
    /// # Arguments
    ///
    /// * `sensor_name` - The name of the sensor to monitor.
    /// * `strategy` - The sampling strategy (e.g., "event", "period", "none").
    /// * `params` - Additional parameters required by the strategy.
    /// * `timeout` - How long to wait for the command to succeed.
    /// * `callback` - The closure to execute when new sensor data arrives.
    ///
    /// # Returns
    ///
    /// `Ok(Message)` containing the success reply from the server.
    ///
    /// # Errors
    ///
    /// Returns a `ClientError` on timeout or disconnect.
    ///
    /// # Example
    /// ```ignore
    /// client.set_sensor_sampling("temperature", "event", &[], Duration::from_secs(5),
    ///     |update| {
    ///         println!("{}: {} = {:?}", update.name, update.status.as_str(),
    ///             update.value_str());
    ///     },
    /// ).await?;
    /// ```
    pub async fn set_sensor_sampling<F>(
        &self,
        sensor_name: &str,
        strategy: &str,
        params: &[&str],
        timeout: Duration,
        callback: F,
    ) -> Result<Message, ClientError>
    where
        F: Fn(&SensorUpdate) + Send + Sync + 'static,
    {
        // Build request args: sensor_name, strategy, params...
        let mut args: Vec<&str> = vec![sensor_name, strategy];
        args.extend_from_slice(params);

        let (reply, _) = self.request_str("sensor-sampling", &args, timeout).await?;

        if reply.is_ok() {
            // Register the per-sensor callback
            self.sensor_callbacks
                .write()
                .await
                .insert(sensor_name.to_string(), Arc::new(callback));
        }

        Ok(reply)
    }

    /// Clear sensor sampling for a specific sensor and remove its callback.
    ///
    /// Sends `?sensor-sampling <sensor_name> none` to the server.
    ///
    /// # Arguments
    ///
    /// * `sensor_name` - The name of the sensor to stop sampling.
    /// * `timeout` - Timeout for the server request.
    ///
    /// # Returns
    ///
    /// The reply message indicating success or failure.
    pub async fn clear_sensor_sampling(
        &self,
        sensor_name: &str,
        timeout: Duration,
    ) -> Result<Message, ClientError> {
        let (reply, _) = self
            .request_str("sensor-sampling", &[sensor_name, "none"], timeout)
            .await?;

        if reply.is_ok() {
            self.sensor_callbacks.write().await.remove(sensor_name);
        }

        Ok(reply)
    }

    /// Wait for the protocol version to be received from the server.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Time to wait before giving up.
    ///
    /// # Returns
    ///
    /// Contains the connected protocol version details.
    pub async fn wait_protocol(&self, timeout: Duration) -> Result<ProtocolFlags, ClientError> {
        let start = tokio::time::Instant::now();
        loop {
            if let Some(ref pf) = *self.protocol.read().await {
                return Ok(pf.clone());
            }
            if start.elapsed() >= timeout {
                return Err(ClientError::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Check if the client is connected.
    ///
    /// # Returns
    ///
    /// `true` if connected, `false` otherwise.
    pub async fn is_connected(&self) -> bool {
        *self.connected.read().await
    }

    /// Receive the next unsolicited inform message.
    ///
    /// # Returns
    ///
    /// The next `Message` that is not a reply to any pending request, or `None` if the queue is closed.
    pub async fn recv_inform(&self) -> Option<Message> {
        self.unsolicited_rx.lock().await.recv().await
    }

    /// Disconnect from the server.
    pub async fn disconnect(&self) {
        *self.write_tx.lock().await = None;
        *self.connected.write().await = false;
    }
}

/// Client errors.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("not connected")]
    NotConnected,
    #[error("request timed out")]
    Timeout,
    #[error("channel closed")]
    ChannelClosed,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::DeviceServer;
    use crate::sensor::{Sensor, SensorStatus};
    use tokio::net::TcpListener;

    async fn setup() -> (Arc<DeviceServer>, u16, mpsc::Sender<()>) {
        let server = Arc::new(DeviceServer::new(
            "127.0.0.1",
            0,
            ("test-server", 0, 1),
            ("test-build", 0, 1, "dev"),
        ));

        // Add a test sensor
        server
            .add_sensor(Sensor::integer(
                "test-sensor",
                "A test sensor",
                "count",
                Some(0),
                Some(100),
                0,
            ))
            .await;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let context = server.context().clone();
        let clients_map = {
            // Access the clients field - we need to restructure slightly
            // For tests, we'll start the full server on a known port
            Arc::new(Mutex::new(HashMap::new()))
        };

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        let ctx = context.clone();
        let cl = clients_map.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok((stream, addr)) => {
                                let ctx = ctx.clone();
                                let cl = cl.clone();
                                tokio::spawn(async move {
                                    let _ = crate::server::handle_client(stream, addr, ctx, cl).await;
                                });
                            }
                            Err(_) => break,
                        }
                    }
                    _ = shutdown_rx.recv() => break,
                }
            }
        });

        (server, port, shutdown_tx)
    }

    #[tokio::test]
    async fn test_client_connect_and_protocol() {
        let (_server, port, _shutdown) = setup().await;

        let client = DeviceClient::new("127.0.0.1", port);
        client.connect().await.unwrap();

        let pf = client.wait_protocol(Duration::from_secs(2)).await.unwrap();
        assert_eq!(pf.major, 5);
    }

    #[tokio::test]
    async fn test_client_watchdog() {
        let (_server, port, _shutdown) = setup().await;

        let client = DeviceClient::new("127.0.0.1", port);
        client.connect().await.unwrap();
        client
            .wait_protocol(Duration::from_secs(2))
            .await
            .unwrap();

        let (reply, _) = client
            .request_str("watchdog", &[], Duration::from_secs(2))
            .await
            .unwrap();
        assert!(reply.is_ok());
    }

    #[tokio::test]
    async fn test_client_help() {
        let (_server, port, _shutdown) = setup().await;

        let client = DeviceClient::new("127.0.0.1", port);
        client.connect().await.unwrap();
        client
            .wait_protocol(Duration::from_secs(2))
            .await
            .unwrap();

        let (reply, informs) = client
            .request_str("help", &[], Duration::from_secs(2))
            .await
            .unwrap();
        assert!(reply.is_ok());
        assert!(!informs.is_empty());
    }

    #[tokio::test]
    async fn test_client_sensor_list() {
        let (_server, port, _shutdown) = setup().await;

        let client = DeviceClient::new("127.0.0.1", port);
        client.connect().await.unwrap();
        client
            .wait_protocol(Duration::from_secs(2))
            .await
            .unwrap();

        let (reply, informs) = client
            .request_str("sensor-list", &[], Duration::from_secs(2))
            .await
            .unwrap();
        assert!(reply.is_ok());
        assert_eq!(informs.len(), 1);
        assert_eq!(informs[0].arg_str(0), Some("test-sensor"));
    }

    #[tokio::test]
    async fn test_client_sensor_value() {
        let (server, port, _shutdown) = setup().await;

        // Set sensor value
        {
            let sensors = server.context().sensors.read().await;
            sensors
                .get("test-sensor")
                .unwrap()
                .set_int(42, SensorStatus::Nominal);
        }

        let client = DeviceClient::new("127.0.0.1", port);
        client.connect().await.unwrap();
        client
            .wait_protocol(Duration::from_secs(2))
            .await
            .unwrap();

        let (reply, informs) = client
            .request_str("sensor-value", &["test-sensor"], Duration::from_secs(2))
            .await
            .unwrap();
        assert!(reply.is_ok());
        assert_eq!(informs.len(), 1);
        // informs: timestamp, count, name, status, value
        assert_eq!(informs[0].arg_str(2), Some("test-sensor"));
        assert_eq!(informs[0].arg_str(3), Some("nominal"));
        assert_eq!(informs[0].arg_str(4), Some("42"));
    }

    #[tokio::test]
    async fn test_client_unknown_request() {
        let (_server, port, _shutdown) = setup().await;

        let client = DeviceClient::new("127.0.0.1", port);
        client.connect().await.unwrap();
        client
            .wait_protocol(Duration::from_secs(2))
            .await
            .unwrap();

        let (reply, _) = client
            .request_str("nonexistent", &[], Duration::from_secs(2))
            .await
            .unwrap();
        assert!(!reply.is_ok());
        assert_eq!(reply.reply_status(), Some("invalid"));
    }
}
