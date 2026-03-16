use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex, RwLock};

use regex::Regex;

use crate::message::{Message, MessageParser, MessageType};
use crate::protocol::ProtocolFlags;
use crate::sampling::{SamplingObserver, SamplingStrategy};
use crate::sensor::Sensor;

/// A request handler function type.
/// Takes the request message and returns a list of (inform messages, reply message).
pub type RequestHandler =
    Arc<dyn Fn(&Message, &ServerContext) -> RequestResult + Send + Sync + 'static>;

/// Result of handling a request.
pub struct RequestResult {
    pub informs: Vec<Message>,
    pub reply: Message,
}

/// Valid KATCP log levels.
const VALID_LOG_LEVELS: &[&str] = &["all", "trace", "debug", "info", "warn", "error", "fatal", "off"];

/// Shared server context available to request handlers.
pub struct ServerContext {
    pub sensors: Arc<RwLock<HashMap<String, Sensor>>>,
    pub protocol: ProtocolFlags,
    pub version_info: (String, u32, u32),
    pub build_info: (String, u32, u32, String),
    handlers: Arc<RwLock<HashMap<String, RequestHandler>>>,
    log_level: RwLock<String>,
}

/// Per-client state.
pub(crate) struct ClientState {
    addr: SocketAddr,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    sampling: HashMap<String, SamplingStrategy>,
}

/// A KATCP device server.
pub struct DeviceServer {
    bind_addr: String,
    port: u16,
    context: Arc<ServerContext>,
    pub(crate) clients: Arc<Mutex<HashMap<SocketAddr, ClientState>>>,
}

impl DeviceServer {
    /// Create a new device server.
    ///
    /// # Arguments
    ///
    /// * `host` - The address to interface with (e.g., "0.0.0.0" or "127.0.0.1"). If empty, defaults to "0.0.0.0".
    /// * `port` - The port to listen on. Port 0 asks the OS to assign a free port.
    /// * `version_info` - A tuple of `(name, major, minor)` describing the device.
    /// * `build_info` - A tuple of `(name, major, minor, build_state)` describing the device build.
    ///
    /// # Returns
    ///
    /// A newly instantiated `DeviceServer`.
    pub fn new(
        host: &str,
        port: u16,
        version_info: (&str, u32, u32),
        build_info: (&str, u32, u32, &str),
    ) -> Self {
        let protocol = ProtocolFlags::default_v5();

        let context = Arc::new(ServerContext {
            sensors: Arc::new(RwLock::new(HashMap::new())),
            protocol,
            version_info: (version_info.0.to_string(), version_info.1, version_info.2),
            build_info: (
                build_info.0.to_string(),
                build_info.1,
                build_info.2,
                build_info.3.to_string(),
            ),
            handlers: Arc::new(RwLock::new(HashMap::new())),
            log_level: RwLock::new("warn".to_string()),
        });

        Self {
            bind_addr: if host.is_empty() {
                "0.0.0.0".to_string()
            } else {
                host.to_string()
            },
            port,
            context,
            clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get a reference to the server context for adding sensors/handlers before starting.
    ///
    /// # Returns
    ///
    /// An `Arc` wrapping the shared `ServerContext`.
    pub fn context(&self) -> &Arc<ServerContext> {
        &self.context
    }

    /// Add a sensor to the server.
    ///
    /// # Arguments
    ///
    /// * `sensor` - The `Sensor` instance to register with the server and make available to clients.
    pub async fn add_sensor(&self, sensor: Sensor) {
        self.context
            .sensors
            .write()
            .await
            .insert(sensor.name.clone(), sensor);
    }

    /// Register a custom request handler.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the custom request (without the leading '?').
    /// * `handler` - A closure or function that takes the `Message` and `ServerContext` and returns a `RequestResult`.
    pub async fn register_request_handler<F>(&self, name: &str, handler: F)
    where
        F: Fn(&Message, &ServerContext) -> RequestResult + Send + Sync + 'static,
    {
        self.context
            .handlers
            .write()
            .await
            .insert(name.to_string(), Arc::new(handler));
    }

    /// Bind the server socket and return the listener and actual bound address.
    /// Useful for getting the port before starting (e.g. when using port 0).
    ///
    /// # Returns
    ///
    /// `Ok((listener, address))` upon successfully binding the socket.
    ///
    /// # Errors
    ///
    /// Returns an `std::io::Error` if binding to the address and port fails.
    pub async fn bind(&self) -> std::io::Result<(TcpListener, std::net::SocketAddr)> {
        let addr = format!("{}:{}", self.bind_addr, self.port);
        let listener = TcpListener::bind(&addr).await?;
        let local_addr = listener.local_addr()?;
        Ok((listener, local_addr))
    }

    /// Start the server and run until the shutdown signal is received.
    ///
    /// # Arguments
    ///
    /// * `shutdown` - A receiver for a shutdown signal. Send `()` to this channel to stop the server gracefully.
    ///
    /// # Returns
    ///
    /// `Ok(())` when gracefully shut down.
    ///
    /// # Errors
    ///
    /// Returns an `std::io::Error` if there's a problem establishing the basic socket listener.
    pub async fn run(&self, mut shutdown: mpsc::Receiver<()>) -> std::io::Result<()> {
        let (listener, _) = self.bind().await?;
        self.run_with_listener(listener, &mut shutdown).await
    }

    /// Run the server with a pre-bound listener until the shutdown signal is received.
    ///
    /// # Arguments
    ///
    /// * `listener` - An already-bound `TcpListener`.
    /// * `shutdown` - A mutable reference to a receiver for the shutdown signal.
    ///
    /// # Returns
    ///
    /// `Ok(())` upon completing execution and gracefully shutting down.
    ///
    /// # Errors
    ///
    /// Returns an `std::io::Error` if a critical socket error occurs unexpectedly.
    pub async fn run_with_listener(
        &self,
        listener: TcpListener,
        shutdown: &mut mpsc::Receiver<()>,
    ) -> std::io::Result<()> {
        tracing::info!("KATCP server listening on {}", listener.local_addr()?);

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            tracing::info!("Client connected: {}", addr);
                            let context = self.context.clone();
                            let clients = self.clients.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_client(stream, addr, context, clients).await {
                                    tracing::error!("Client {} error: {}", addr, e);
                                }
                            });
                        }
                        Err(e) => {
                            tracing::error!("Accept error: {}", e);
                        }
                    }
                }
                _ = shutdown.recv() => {
                    tracing::info!("Server shutting down");
                    // Notify all connected clients
                    let clients = self.clients.lock().await;
                    for (_, client) in clients.iter() {
                        let disconnect = Message::inform_with_str_args(
                            "disconnect", &["server shutting down"]
                        );
                        let _ = client.tx.send(disconnect.to_bytes());
                    }
                    break;
                }
            }
        }
        Ok(())
    }

    /// Start the server, returning a handle that can be used to shut it down.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - The `JoinHandle` representing the background server task.
    /// - A `mpsc::Sender` that can be used to send a shutdown signal to the server.
    pub fn start(
        self: Arc<Self>,
    ) -> (
        tokio::task::JoinHandle<std::io::Result<()>>,
        mpsc::Sender<()>,
    ) {
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let handle = tokio::spawn(async move { self.run(shutdown_rx).await });
        (handle, shutdown_tx)
    }
}

/// Handle a single connected client, reading messages and dispatching them.
pub(crate) async fn handle_client(
    stream: TcpStream,
    addr: SocketAddr,
    context: Arc<ServerContext>,
    clients: Arc<Mutex<HashMap<SocketAddr, ClientState>>>,
) -> std::io::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Create channel for sending messages to this client
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();

    // Register client
    {
        let mut clients = clients.lock().await;
        clients.insert(
            addr,
            ClientState {
                addr,
                tx: tx.clone(),
                sampling: HashMap::new(),
            },
        );
    }

    // Send version-connect informs
    let version_informs = build_version_informs(&context);
    for inform in &version_informs {
        let bytes = inform.to_bytes();
        writer.write_all(&bytes).await?;
    }

    // Spawn writer task
    let write_handle = tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if writer.write_all(&data).await.is_err() {
                break;
            }
        }
    });

    // Read and process messages
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
                        if msg.mtype == MessageType::Request {
                            let result =
                                handle_request(&msg, &context, &clients, addr, &tx).await;
                            // Send informs first, then reply
                            for inform in &result.informs {
                                let _ = tx.send(inform.to_bytes());
                            }
                            let _ = tx.send(result.reply.to_bytes());
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Parse error from {}: {}", addr, e);
                    }
                }
            }
            Err(e) => {
                tracing::error!("Read error from {}: {}", addr, e);
                break;
            }
        }
    }

    // Cleanup: detach only this client's observers, leaving other clients' intact
    tracing::info!("Client disconnected: {}", addr);
    {
        let mut clients_lock = clients.lock().await;
        if let Some(client_state) = clients_lock.get(&addr) {
            let sensor_names: Vec<String> = client_state.sampling.keys().cloned().collect();
            let sensors = context.sensors.read().await;
            for name in sensor_names {
                if let Some(sensor) = sensors.get(&name) {
                    sensor.detach_client(addr);
                }
            }
        }
        clients_lock.remove(&addr);
    }
    drop(tx);
    let _ = write_handle.await;
    Ok(())
}

/// Build the standard version-connect inform messages sent upon connection.
fn build_version_informs(context: &ServerContext) -> Vec<Message> {
    let proto_str = context.protocol.to_string();
    let version_str = format!(
        "{}-{}.{}",
        context.version_info.0, context.version_info.1, context.version_info.2
    );
    let build_str = format!(
        "{}-{}.{}-{}",
        context.build_info.0, context.build_info.1, context.build_info.2, context.build_info.3
    );

    vec![
        Message::inform(
            "version-connect",
            vec![b"katcp-protocol".to_vec(), proto_str.into_bytes()],
        ),
        Message::inform(
            "version-connect",
            vec![
                b"katcp-library".to_vec(),
                format!("katcp-rust-{}", env!("CARGO_PKG_VERSION")).into_bytes(),
            ],
        ),
        Message::inform(
            "version-connect",
            vec![
                b"katcp-device".to_vec(),
                version_str.into_bytes(),
                build_str.into_bytes(),
            ],
        ),
    ]
}

/// Route an incoming request message to the appropriate handler.
async fn handle_request(
    msg: &Message,
    context: &ServerContext,
    clients: &Arc<Mutex<HashMap<SocketAddr, ClientState>>>,
    client_addr: SocketAddr,
    client_tx: &mpsc::UnboundedSender<Vec<u8>>,
) -> RequestResult {
    // Check for custom handlers first
    {
        let handlers = context.handlers.read().await;
        if let Some(handler) = handlers.get(&msg.name) {
            return handler(msg, context);
        }
    }

    // Built-in request handlers
    match msg.name.as_str() {
        "help" => handle_help(msg, context).await,
        "watchdog" => handle_watchdog(msg),
        "version-list" => handle_version_list(msg, context),
        "sensor-list" => handle_sensor_list(msg, context).await,
        "sensor-value" => handle_sensor_value(msg, context).await,
        "sensor-sampling" => {
            handle_sensor_sampling(msg, context, clients, client_addr, client_tx).await
        }
        "client-list" => handle_client_list(msg, clients).await,
        "halt" => handle_halt(msg),
        "log-level" => handle_log_level(msg, context).await,
        _ => RequestResult {
            informs: vec![],
            reply: Message::reply_to_request(
                msg,
                vec![b"invalid".to_vec(), b"unknown request".to_vec()],
            ),
        },
    }
}

/// Handle a `?watchdog` request, responding with an `!ok`.
fn handle_watchdog(msg: &Message) -> RequestResult {
    RequestResult {
        informs: vec![],
        reply: Message::reply_to_request(msg, vec![b"ok".to_vec()]),
    }
}

/// Handle a `?help` request, returning available commands and their descriptions.
async fn handle_help(msg: &Message, context: &ServerContext) -> RequestResult {
    let builtin = vec![
        ("help", "List available requests."),
        ("watchdog", "Ping the server."),
        ("version-list", "List server version information."),
        ("sensor-list", "List available sensors."),
        ("sensor-value", "Get sensor value."),
        ("sensor-sampling", "Set sensor sampling strategy."),
        ("client-list", "List connected clients."),
        ("halt", "Shut down the server."),
        ("log-level", "Get or set log level."),
    ];

    if let Some(name) = msg.arg_str(0) {
        // Help for a specific request
        let handlers = context.handlers.read().await;
        for (n, desc) in &builtin {
            if *n == name {
                return RequestResult {
                    informs: vec![Message::reply_inform(
                        msg,
                        vec![name.as_bytes().to_vec(), desc.as_bytes().to_vec()],
                    )],
                    reply: Message::reply_to_request(msg, vec![b"ok".to_vec(), b"1".to_vec()]),
                };
            }
        }
        if handlers.contains_key(name) {
            return RequestResult {
                informs: vec![Message::reply_inform(
                    msg,
                    vec![
                        name.as_bytes().to_vec(),
                        b"Custom request handler.".to_vec(),
                    ],
                )],
                reply: Message::reply_to_request(msg, vec![b"ok".to_vec(), b"1".to_vec()]),
            };
        }
        return RequestResult {
            informs: vec![],
            reply: Message::reply_to_request(
                msg,
                vec![b"fail".to_vec(), b"Unknown request.".to_vec()],
            ),
        };
    }

    // List all requests
    let handlers = context.handlers.read().await;
    let mut informs: Vec<Message> = builtin
        .iter()
        .map(|(name, desc)| {
            Message::reply_inform(
                msg,
                vec![name.as_bytes().to_vec(), desc.as_bytes().to_vec()],
            )
        })
        .collect();

    for name in handlers.keys() {
        informs.push(Message::reply_inform(
            msg,
            vec![
                name.as_bytes().to_vec(),
                b"Custom request handler.".to_vec(),
            ],
        ));
    }

    let count = informs.len().to_string();
    RequestResult {
        informs,
        reply: Message::reply_to_request(msg, vec![b"ok".to_vec(), count.into_bytes()]),
    }
}

/// Handle a `?version-list` request, returning version info for protocol, library, and device.
fn handle_version_list(msg: &Message, context: &ServerContext) -> RequestResult {
    let proto_str = context.protocol.to_string();
    let version_str = format!(
        "{}-{}.{}",
        context.version_info.0, context.version_info.1, context.version_info.2
    );
    let build_str = format!(
        "{}-{}.{}-{}",
        context.build_info.0, context.build_info.1, context.build_info.2, context.build_info.3
    );
    let lib_str = format!("katcp-rust-{}", env!("CARGO_PKG_VERSION"));

    let informs = vec![
        Message::reply_inform(
            msg,
            vec![b"katcp-protocol".to_vec(), proto_str.into_bytes()],
        ),
        Message::reply_inform(
            msg,
            vec![
                b"katcp-library".to_vec(),
                lib_str.into_bytes(),
                build_str.clone().into_bytes(),
            ],
        ),
        Message::reply_inform(
            msg,
            vec![
                b"katcp-device".to_vec(),
                version_str.into_bytes(),
                build_str.into_bytes(),
            ],
        ),
    ];

    let count = informs.len().to_string();
    RequestResult {
        informs,
        reply: Message::reply_to_request(
            msg,
            vec![b"ok".to_vec(), count.into_bytes()],
        ),
    }
}

/// Sensor name filter: exact name, regex pattern `/pattern/`, or all (None).
enum SensorFilter<'a> {
    All,
    Exact(&'a str),
    Regex(Regex),
}

impl<'a> SensorFilter<'a> {
    /// Parse a sensor filter string.
    fn parse(arg: Option<&'a str>) -> Result<Self, String> {
        match arg {
            None => Ok(SensorFilter::All),
            Some(s) if s.starts_with('/') && s.ends_with('/') && s.len() > 1 => {
                let pattern = &s[1..s.len() - 1];
                Regex::new(pattern)
                    .map(SensorFilter::Regex)
                    .map_err(|e| format!("Invalid regex: {}", e))
            }
            Some(s) => Ok(SensorFilter::Exact(s)),
        }
    }

    /// Check if a sensor name matches the filter.
    fn matches(&self, name: &str) -> bool {
        match self {
            SensorFilter::All => true,
            SensorFilter::Exact(n) => name == *n,
            SensorFilter::Regex(re) => re.is_match(name),
        }
    }

}

/// Build a `#sensor-list` inform for a given sensor.
fn build_sensor_list_inform(msg: &Message, sensor: &Sensor) -> Message {
    let params = sensor.params_str();
    let mut args = vec![
        sensor.name.as_bytes().to_vec(),
        sensor.description.as_bytes().to_vec(),
        sensor.units.as_bytes().to_vec(),
        sensor.sensor_type.as_str().as_bytes().to_vec(),
    ];
    if !params.is_empty() {
        for p in params.split_whitespace() {
            args.push(p.as_bytes().to_vec());
        }
    }
    Message::reply_inform(msg, args)
}

/// Handle a `?sensor-list` request.
async fn handle_sensor_list(msg: &Message, context: &ServerContext) -> RequestResult {
    let sensors = context.sensors.read().await;

    let filter = match SensorFilter::parse(msg.arg_str(0)) {
        Ok(f) => f,
        Err(e) => {
            return RequestResult {
                informs: vec![],
                reply: Message::reply_to_request(
                    msg,
                    vec![b"fail".to_vec(), e.into_bytes()],
                ),
            };
        }
    };

    // For exact name, check existence and fail if not found
    if let SensorFilter::Exact(name) = &filter {
        return match sensors.get(*name) {
            Some(sensor) => {
                let inform = build_sensor_list_inform(msg, sensor);
                RequestResult {
                    informs: vec![inform],
                    reply: Message::reply_to_request(
                        msg,
                        vec![b"ok".to_vec(), b"1".to_vec()],
                    ),
                }
            }
            None => RequestResult {
                informs: vec![],
                reply: Message::reply_to_request(
                    msg,
                    vec![b"fail".to_vec(), b"Unknown sensor name.".to_vec()],
                ),
            },
        };
    }

    let mut informs = Vec::new();
    for sensor in sensors.values() {
        if filter.matches(&sensor.name) {
            informs.push(build_sensor_list_inform(msg, sensor));
        }
    }

    let count = informs.len().to_string();
    RequestResult {
        informs,
        reply: Message::reply_to_request(msg, vec![b"ok".to_vec(), count.into_bytes()]),
    }
}

/// Build a `#sensor-value` inform for a given sensor.
fn build_sensor_value_inform(msg: &Message, sensor: &Sensor) -> Message {
    let (ts, status, value) = sensor.read_formatted();
    Message::reply_inform(
        msg,
        vec![
            ts,
            b"1".to_vec(),
            sensor.name.as_bytes().to_vec(),
            status,
            value,
        ],
    )
}

/// Handle a `?sensor-value` request.
async fn handle_sensor_value(msg: &Message, context: &ServerContext) -> RequestResult {
    let sensors = context.sensors.read().await;

    let filter = match SensorFilter::parse(msg.arg_str(0)) {
        Ok(f) => f,
        Err(e) => {
            return RequestResult {
                informs: vec![],
                reply: Message::reply_to_request(
                    msg,
                    vec![b"fail".to_vec(), e.into_bytes()],
                ),
            };
        }
    };

    // For exact name, check existence and fail if not found
    if let SensorFilter::Exact(name) = &filter {
        return match sensors.get(*name) {
            Some(sensor) => {
                let inform = build_sensor_value_inform(msg, sensor);
                RequestResult {
                    informs: vec![inform],
                    reply: Message::reply_to_request(
                        msg,
                        vec![b"ok".to_vec(), b"1".to_vec()],
                    ),
                }
            }
            None => RequestResult {
                informs: vec![],
                reply: Message::reply_to_request(
                    msg,
                    vec![b"fail".to_vec(), b"Unknown sensor name.".to_vec()],
                ),
            },
        };
    }

    // All or regex — return matching sensors, always ok
    let mut informs = Vec::new();
    for sensor in sensors.values() {
        if filter.matches(&sensor.name) {
            informs.push(build_sensor_value_inform(msg, sensor));
        }
    }
    let count = informs.len().to_string();
    RequestResult {
        informs,
        reply: Message::reply_to_request(msg, vec![b"ok".to_vec(), count.into_bytes()]),
    }
}

/// Handle a `?sensor-sampling` request.
async fn handle_sensor_sampling(
    msg: &Message,
    context: &ServerContext,
    clients: &Arc<Mutex<HashMap<SocketAddr, ClientState>>>,
    client_addr: SocketAddr,
    client_tx: &mpsc::UnboundedSender<Vec<u8>>,
) -> RequestResult {
    let sensor_name = match msg.arg_str(0) {
        Some(name) => name.to_string(),
        None => {
            return RequestResult {
                informs: vec![],
                reply: Message::reply_to_request(
                    msg,
                    vec![b"fail".to_vec(), b"sensor name required".to_vec()],
                ),
            };
        }
    };

    // Verify sensor exists
    {
        let sensors = context.sensors.read().await;
        if !sensors.contains_key(&sensor_name) {
            return RequestResult {
                informs: vec![],
                reply: Message::reply_to_request(
                    msg,
                    vec![b"fail".to_vec(), b"Unknown sensor name.".to_vec()],
                ),
            };
        }
    }

    // Query mode: if no strategy arg, return current sampling for this sensor
    if msg.arg_str(1).is_none() {
        let clients_lock = clients.lock().await;
        let current = clients_lock
            .get(&client_addr)
            .and_then(|c| c.sampling.get(&sensor_name).cloned())
            .unwrap_or(SamplingStrategy::None);
        let params_str = current.params_str();
        let mut reply_args = vec![
            b"ok".to_vec(),
            sensor_name.into_bytes(),
            current.as_str().as_bytes().to_vec(),
        ];
        if !params_str.is_empty() {
            for p in params_str.split_whitespace() {
                reply_args.push(p.as_bytes().to_vec());
            }
        }
        return RequestResult {
            informs: vec![],
            reply: Message::reply_to_request(msg, reply_args),
        };
    }

    let strategy_name = msg.arg_str(1).unwrap();
    let params: Vec<&str> = (2..msg.arguments.len())
        .filter_map(|i| msg.arg_str(i))
        .collect();

    let strategy = match SamplingStrategy::parse(strategy_name, &params) {
        Some(s) => s,
        None => {
            return RequestResult {
                informs: vec![],
                reply: Message::reply_to_request(
                    msg,
                    vec![b"fail".to_vec(), b"invalid sampling strategy".to_vec()],
                ),
            };
        }
    };

    let sensors = context.sensors.read().await;
    let sensor = sensors.get(&sensor_name).unwrap(); // already verified above

    // Detach only this client's observer for this sensor
    sensor.detach_client(client_addr);

    // Attach new observer if strategy is not None
    if strategy != SamplingStrategy::None {
        let observer = SamplingObserver::new(sensor_name.clone(), strategy, client_addr, client_tx.clone());
        sensor.attach(observer);
    }

    // Update client state
    {
        let mut clients = clients.lock().await;
        if let Some(client) = clients.get_mut(&client_addr) {
            if strategy == SamplingStrategy::None {
                client.sampling.remove(&sensor_name);
            } else {
                client.sampling.insert(sensor_name.clone(), strategy);
            }
        }
    }

    let params_str = strategy.params_str();
    let mut reply_args = vec![
        b"ok".to_vec(),
        sensor_name.into_bytes(),
        strategy.as_str().as_bytes().to_vec(),
    ];
    if !params_str.is_empty() {
        for p in params_str.split_whitespace() {
            reply_args.push(p.as_bytes().to_vec());
        }
    }

    RequestResult {
        informs: vec![],
        reply: Message::reply_to_request(msg, reply_args),
    }
}

/// Handle a `?client-list` request.
async fn handle_client_list(
    msg: &Message,
    clients: &Arc<Mutex<HashMap<SocketAddr, ClientState>>>,
) -> RequestResult {
    let clients = clients.lock().await;
    let informs: Vec<Message> = clients
        .values()
        .map(|c| {
            Message::reply_inform(msg, vec![c.addr.to_string().into_bytes()])
        })
        .collect();
    let count = informs.len().to_string();

    RequestResult {
        informs,
        reply: Message::reply_to_request(msg, vec![b"ok".to_vec(), count.into_bytes()]),
    }
}

/// Handle a `?halt` request.
fn handle_halt(msg: &Message) -> RequestResult {
    RequestResult {
        informs: vec![],
        reply: Message::reply_to_request(msg, vec![b"ok".to_vec()]),
    }
}

/// Handle a `?log-level` request.
async fn handle_log_level(msg: &Message, context: &ServerContext) -> RequestResult {
    match msg.arg_str(0) {
        None => {
            // Query mode: return current log level
            let level = context.log_level.read().await;
            RequestResult {
                informs: vec![],
                reply: Message::reply_to_request(
                    msg,
                    vec![b"ok".to_vec(), level.as_bytes().to_vec()],
                ),
            }
        }
        Some(level) => {
            if VALID_LOG_LEVELS.contains(&level) {
                *context.log_level.write().await = level.to_string();
                RequestResult {
                    informs: vec![],
                    reply: Message::reply_to_request(
                        msg,
                        vec![b"ok".to_vec(), level.as_bytes().to_vec()],
                    ),
                }
            } else {
                RequestResult {
                    informs: vec![],
                    reply: Message::reply_to_request(
                        msg,
                        vec![b"fail".to_vec(), b"Invalid log level.".to_vec()],
                    ),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{BufReader as TokioBufReader};

    async fn start_test_server() -> (mpsc::Sender<()>, u16) {
        let server = Arc::new(DeviceServer::new(
            "127.0.0.1",
            0,
            ("test-server", 0, 1),
            ("test-build", 0, 1, "dev"),
        ));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let context = server.context().clone();
        let clients = server.clients.clone();
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = listener.accept() => {
                        match result {
                            Ok((stream, addr)) => {
                                let ctx = context.clone();
                                let cl = clients.clone();
                                tokio::spawn(async move {
                                    let _ = handle_client(stream, addr, ctx, cl).await;
                                });
                            }
                            Err(_) => break,
                        }
                    }
                    _ = shutdown_rx.recv() => break,
                }
            }
        });

        (shutdown_tx, port)
    }

    /// Read lines until we find one containing `needle`, or until `count` lines read.
    async fn read_until_contains(
        reader: &mut TokioBufReader<tokio::net::tcp::OwnedReadHalf>,
        needle: &str,
        max_lines: usize,
    ) -> String {
        let mut all = String::new();
        for _ in 0..max_lines {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            all.push_str(&line);
            if all.contains(needle) {
                return all;
            }
        }
        all
    }

    /// Read exactly `count` lines from the reader.
    async fn read_lines(
        reader: &mut TokioBufReader<tokio::net::tcp::OwnedReadHalf>,
        count: usize,
    ) -> String {
        let mut all = String::new();
        for _ in 0..count {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            all.push_str(&line);
        }
        all
    }

    #[tokio::test]
    async fn test_server_version_connect() {
        let (_shutdown, port) = start_test_server().await;

        let stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let (reader, _writer) = stream.into_split();
        let mut reader = TokioBufReader::new(reader);

        // Server sends 3 lines on connect (3 version-connect informs)
        let response = read_lines(&mut reader, 3).await;

        assert!(response.contains("#version-connect katcp-protocol"));
        assert!(response.contains("#version-connect katcp-library"));
        assert!(response.contains("#version-connect katcp-device"));
    }

    #[tokio::test]
    async fn test_server_watchdog() {
        let (_shutdown, port) = start_test_server().await;

        let stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = TokioBufReader::new(reader);

        // Read version informs
        let _ = read_lines(&mut reader, 3).await;

        // Send watchdog
        writer.write_all(b"?watchdog\n").await.unwrap();
        let response = read_lines(&mut reader, 1).await;
        assert!(response.contains("!watchdog ok"));
    }

    #[tokio::test]
    async fn test_server_help() {
        let (_shutdown, port) = start_test_server().await;

        let stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = TokioBufReader::new(reader);

        let _ = read_lines(&mut reader, 3).await;

        writer.write_all(b"?help\n").await.unwrap();
        // help returns multiple informs + 1 reply
        let response = read_until_contains(&mut reader, "!help ok", 20).await;
        assert!(response.contains("#help"));
        assert!(response.contains("!help ok"));
    }

    #[tokio::test]
    async fn test_server_unknown_request() {
        let (_shutdown, port) = start_test_server().await;

        let stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = TokioBufReader::new(reader);

        let _ = read_lines(&mut reader, 3).await;

        writer.write_all(b"?nonexistent\n").await.unwrap();
        let response = read_lines(&mut reader, 1).await;
        assert!(response.contains("!nonexistent invalid"));
    }

    #[tokio::test]
    async fn test_server_with_mid() {
        let (_shutdown, port) = start_test_server().await;

        let stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = TokioBufReader::new(reader);

        let _ = read_lines(&mut reader, 3).await;

        writer.write_all(b"?watchdog[42]\n").await.unwrap();
        let response = read_lines(&mut reader, 1).await;
        assert!(response.contains("!watchdog[42] ok"));
    }
}
