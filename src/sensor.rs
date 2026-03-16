use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::types::KatcpType;

/// Sensor type identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorType {
    Integer,
    Float,
    Boolean,
    Lru,
    Discrete,
    String,
    Timestamp,
    Address,
}

impl SensorType {
    /// Get the string representation of the sensor type.
    pub fn as_str(&self) -> &'static str {
        match self {
            SensorType::Integer => "integer",
            SensorType::Float => "float",
            SensorType::Boolean => "boolean",
            SensorType::Lru => "lru",
            SensorType::Discrete => "discrete",
            SensorType::String => "string",
            SensorType::Timestamp => "timestamp",
            SensorType::Address => "address",
        }
    }
}

/// Sensor status values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorStatus {
    Unknown,
    Nominal,
    Warn,
    Error,
    Failure,
    Unreachable,
    Inactive,
}

impl SensorStatus {
    /// Get the string representation of the sensor status.
    pub fn as_str(&self) -> &'static str {
        match self {
            SensorStatus::Unknown => "unknown",
            SensorStatus::Nominal => "nominal",
            SensorStatus::Warn => "warn",
            SensorStatus::Error => "error",
            SensorStatus::Failure => "failure",
            SensorStatus::Unreachable => "unreachable",
            SensorStatus::Inactive => "inactive",
        }
    }

    /// Parse a sensor status from its string representation.
    ///
    /// # Arguments
    ///
    /// * `s` - The string representation of the status (e.g., "nominal").
    ///
    /// # Returns
    ///
    /// `Some(SensorStatus)` if the string matches a known status, `None` otherwise.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "unknown" => Some(SensorStatus::Unknown),
            "nominal" => Some(SensorStatus::Nominal),
            "warn" => Some(SensorStatus::Warn),
            "error" => Some(SensorStatus::Error),
            "failure" => Some(SensorStatus::Failure),
            "unreachable" => Some(SensorStatus::Unreachable),
            "inactive" => Some(SensorStatus::Inactive),
            _ => None,
        }
    }
}

/// A timestamped sensor reading.
#[derive(Debug, Clone)]
pub struct SensorReading {
    pub timestamp: f64,
    pub status: SensorStatus,
    pub value: Vec<u8>,
}

impl SensorReading {
    /// Create a new sensor reading.
    ///
    /// # Arguments
    ///
    /// * `timestamp` - The UNIX timestamp in seconds.
    /// * `status` - The current status of the sensor.
    /// * `value` - The raw byte value of the reading.
    ///
    /// # Returns
    ///
    /// A new `SensorReading` instance.
    pub fn new(timestamp: f64, status: SensorStatus, value: Vec<u8>) -> Self {
        Self {
            timestamp,
            status,
            value,
        }
    }
}

/// Observer trait for sensor updates.
pub trait SensorObserver: Send + Sync {
    /// Called when the sensor value is updated.
    ///
    /// # Arguments
    ///
    /// * `sensor_name` - The name of the sensor that updated.
    /// * `reading` - The new sensor reading.
    fn on_update(&self, sensor_name: &str, reading: &SensorReading);

    /// Return the client address that owns this observer, if any.
    fn client_addr(&self) -> Option<SocketAddr> {
        None
    }
}

/// A KATCP sensor.
#[derive(Clone)]
pub struct Sensor {
    pub name: String,
    pub description: String,
    pub units: String,
    pub sensor_type: SensorType,
    pub katcp_type: KatcpType,
    inner: Arc<Mutex<SensorInner>>,
}

struct SensorInner {
    reading: SensorReading,
    observers: Vec<Arc<dyn SensorObserver>>,
}

impl std::fmt::Debug for Sensor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sensor")
            .field("name", &self.name)
            .field("sensor_type", &self.sensor_type)
            .finish()
    }
}

impl Sensor {
    /// Create a new generic sensor.
    fn new(
        name: &str,
        description: &str,
        units: &str,
        sensor_type: SensorType,
        katcp_type: KatcpType,
        default_value: Vec<u8>,
    ) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            units: units.to_string(),
            sensor_type,
            katcp_type,
            inner: Arc::new(Mutex::new(SensorInner {
                reading: SensorReading::new(0.0, SensorStatus::Unknown, default_value),
                observers: Vec::new(),
            })),
        }
    }

    /// Create an integer sensor.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the sensor.
    /// * `description` - A human-readable description of the sensor.
    /// * `units` - The physical units the sensor measures.
    /// * `min` - An optional lower bound for the sensor's value.
    /// * `max` - An optional upper bound for the sensor's value.
    /// * `default` - The initial value of the sensor.
    ///
    /// # Returns
    ///
    /// A newly instantiated integer `Sensor`.
    pub fn integer(
        name: &str,
        description: &str,
        units: &str,
        min: Option<i64>,
        max: Option<i64>,
        default: i64,
    ) -> Self {
        Self::new(
            name,
            description,
            units,
            SensorType::Integer,
            KatcpType::Int { min, max },
            default.to_string().into_bytes(),
        )
    }

    /// Create a float sensor.
    pub fn float(
        name: &str,
        description: &str,
        units: &str,
        min: Option<f64>,
        max: Option<f64>,
        default: f64,
    ) -> Self {
        Self::new(
            name,
            description,
            units,
            SensorType::Float,
            KatcpType::Float { min, max },
            format!("{:.17e}", default).into_bytes(),
        )
    }

    /// Create a boolean sensor.
    pub fn boolean(name: &str, description: &str, units: &str, default: bool) -> Self {
        Self::new(
            name,
            description,
            units,
            SensorType::Boolean,
            KatcpType::Bool,
            if default {
                b"1".to_vec()
            } else {
                b"0".to_vec()
            },
        )
    }

    /// Create a discrete sensor.
    pub fn discrete(
        name: &str,
        description: &str,
        units: &str,
        values: Vec<String>,
        default: &str,
    ) -> Self {
        Self::new(
            name,
            description,
            units,
            SensorType::Discrete,
            KatcpType::Discrete {
                values: values.clone(),
            },
            default.as_bytes().to_vec(),
        )
    }

    /// Create a string sensor.
    pub fn string(name: &str, description: &str, units: &str, default: &str) -> Self {
        Self::new(
            name,
            description,
            units,
            SensorType::String,
            KatcpType::Str,
            default.as_bytes().to_vec(),
        )
    }

    /// Create a timestamp sensor.
    pub fn timestamp(name: &str, description: &str, units: &str, default: f64) -> Self {
        Self::new(
            name,
            description,
            units,
            SensorType::Timestamp,
            KatcpType::Timestamp,
            format!("{:.6}", default).into_bytes(),
        )
    }

    /// Create an LRU sensor.
    pub fn lru(name: &str, description: &str, units: &str, default: bool) -> Self {
        Self::new(
            name,
            description,
            units,
            SensorType::Lru,
            KatcpType::Lru,
            if default {
                b"nominal".to_vec()
            } else {
                b"error".to_vec()
            },
        )
    }

    /// Get the current timestamp in seconds.
    fn now() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }

    /// Set the sensor value with a given status and optional timestamp.
    ///
    /// # Arguments
    ///
    /// * `value` - The new byte value of the sensor.
    /// * `status` - The new status of the sensor.
    /// * `timestamp` - An optional custom timestamp. If empty, the current time is used.
    pub fn set_value(&self, value: Vec<u8>, status: SensorStatus, timestamp: Option<f64>) {
        let ts = timestamp.unwrap_or_else(Self::now);
        let reading = SensorReading::new(ts, status, value);
        let mut inner = self.inner.lock().unwrap();
        inner.reading = reading.clone();
        let observers: Vec<_> = inner.observers.iter().cloned().collect();
        drop(inner);
        for obs in observers {
            obs.on_update(&self.name, &reading);
        }
    }

    /// Set the sensor value from a formatted integer.
    pub fn set_int(&self, value: i64, status: SensorStatus) {
        self.set_value(value.to_string().into_bytes(), status, None);
    }

    /// Set the sensor value from a formatted float.
    pub fn set_float(&self, value: f64, status: SensorStatus) {
        self.set_value(format!("{:.17e}", value).into_bytes(), status, None);
    }

    /// Set the sensor value from a boolean.
    pub fn set_bool(&self, value: bool, status: SensorStatus) {
        self.set_value(
            if value { b"1".to_vec() } else { b"0".to_vec() },
            status,
            None,
        );
    }

    /// Set the sensor value from a string.
    pub fn set_str(&self, value: &str, status: SensorStatus) {
        self.set_value(value.as_bytes().to_vec(), status, None);
    }

    /// Read the current sensor value.
    ///
    /// # Returns
    ///
    /// A cloned copy of the latest `SensorReading`.
    pub fn read(&self) -> SensorReading {
        self.inner.lock().unwrap().reading.clone()
    }

    /// Read the current sensor value as formatted wire-protocol fields.
    ///
    /// # Returns
    ///
    /// A tuple containing `(timestamp, status, value)` formatted as bytes for KATCP.
    pub fn read_formatted(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let reading = self.read();
        (
            format!("{:.6}", reading.timestamp).into_bytes(),
            reading.status.as_str().as_bytes().to_vec(),
            reading.value,
        )
    }

    /// Attach an observer to this sensor.
    ///
    /// # Arguments
    ///
    /// * `observer` - The observer to be notified when the sensor updates.
    pub fn attach(&self, observer: Arc<dyn SensorObserver>) {
        self.inner.lock().unwrap().observers.push(observer);
    }

    /// Detach all observers belonging to a specific client.
    pub fn detach_client(&self, addr: SocketAddr) {
        self.inner
            .lock()
            .unwrap()
            .observers
            .retain(|obs| obs.client_addr() != Some(addr));
    }

    /// Get the type parameters string for sensor-list (e.g., min/max for int/float).
    ///
    /// # Returns
    ///
    /// A string representation of the sensor's type parameters for use in `#sensor-list`.
    pub fn params_str(&self) -> String {
        match &self.katcp_type {
            KatcpType::Int { min, max } => {
                format!(
                    "{} {}",
                    min.map_or(String::new(), |v| v.to_string()),
                    max.map_or(String::new(), |v| v.to_string())
                )
                .trim()
                .to_string()
            }
            KatcpType::Float { min, max } => {
                format!(
                    "{} {}",
                    min.map_or(String::new(), |v| format!("{:.17e}", v)),
                    max.map_or(String::new(), |v| format!("{:.17e}", v))
                )
                .trim()
                .to_string()
            }
            KatcpType::Discrete { values } => values.join(" "),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_integer_sensor() {
        let s = Sensor::integer("count", "A counter", "count", Some(0), Some(100), 0);
        assert_eq!(s.name, "count");
        assert_eq!(s.sensor_type, SensorType::Integer);

        s.set_int(42, SensorStatus::Nominal);
        let r = s.read();
        assert_eq!(r.status, SensorStatus::Nominal);
        assert_eq!(r.value, b"42");
    }

    #[test]
    fn test_float_sensor() {
        let s = Sensor::float("temp", "Temperature", "C", Some(-40.0), Some(80.0), 20.0);
        s.set_float(25.5, SensorStatus::Nominal);
        let r = s.read();
        assert_eq!(r.status, SensorStatus::Nominal);
        let val: f64 = std::str::from_utf8(&r.value).unwrap().parse().unwrap();
        assert!((val - 25.5).abs() < 1e-10);
    }

    #[test]
    fn test_boolean_sensor() {
        let s = Sensor::boolean("flag", "A flag", "", false);
        s.set_bool(true, SensorStatus::Nominal);
        assert_eq!(s.read().value, b"1");
        s.set_bool(false, SensorStatus::Warn);
        assert_eq!(s.read().value, b"0");
        assert_eq!(s.read().status, SensorStatus::Warn);
    }

    #[test]
    fn test_discrete_sensor() {
        let s = Sensor::discrete(
            "mode",
            "Operating mode",
            "",
            vec!["idle".into(), "running".into(), "error".into()],
            "idle",
        );
        assert_eq!(s.sensor_type, SensorType::Discrete);
        s.set_str("running", SensorStatus::Nominal);
        assert_eq!(s.read().value, b"running");
    }

    #[test]
    fn test_string_sensor() {
        let s = Sensor::string("label", "A label", "", "");
        s.set_str("hello world", SensorStatus::Nominal);
        assert_eq!(s.read().value, b"hello world");
    }

    #[test]
    fn test_read_formatted() {
        let s = Sensor::integer("x", "", "", None, None, 0);
        s.set_value(b"99".to_vec(), SensorStatus::Warn, Some(1234567890.123456));
        let (ts, status, val) = s.read_formatted();
        assert_eq!(ts, b"1234567890.123456");
        assert_eq!(status, b"warn");
        assert_eq!(val, b"99");
    }

    #[test]
    fn test_sensor_observer() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct TestObserver {
            called: AtomicBool,
        }
        impl SensorObserver for TestObserver {
            fn on_update(&self, _name: &str, _reading: &SensorReading) {
                self.called.store(true, Ordering::SeqCst);
            }
        }

        let s = Sensor::integer("x", "", "", None, None, 0);
        let obs = Arc::new(TestObserver {
            called: AtomicBool::new(false),
        });
        s.attach(obs.clone());
        s.set_int(1, SensorStatus::Nominal);
        assert!(obs.called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_sensor_status_roundtrip() {
        for status in [
            SensorStatus::Unknown,
            SensorStatus::Nominal,
            SensorStatus::Warn,
            SensorStatus::Error,
            SensorStatus::Failure,
            SensorStatus::Unreachable,
            SensorStatus::Inactive,
        ] {
            let s = status.as_str();
            assert_eq!(SensorStatus::from_str(s), Some(status));
        }
    }

    #[test]
    fn test_params_str() {
        let s = Sensor::integer("x", "", "", Some(0), Some(100), 0);
        assert_eq!(s.params_str(), "0 100");

        let s = Sensor::discrete("m", "", "", vec!["a".into(), "b".into()], "a");
        assert_eq!(s.params_str(), "a b");

        let s = Sensor::string("s", "", "", "");
        assert_eq!(s.params_str(), "");
    }
}
