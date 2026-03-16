use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::sync::mpsc;

use crate::message::Message;
use crate::sensor::{SensorObserver, SensorReading};

/// Sampling strategy types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SamplingStrategy {
    /// No sampling — sensor updates are not reported.
    None,
    /// Automatic sampling (server chooses strategy).
    Auto,
    /// Sample at fixed period (seconds).
    Period(u64, u32), // (secs, nanos)
    /// Sample on every change.
    Event,
    /// Sample when value changes by at least the given difference.
    Differential(f64),
    /// Rate-limited event sampling: (min_secs, min_nanos, max_secs, max_nanos).
    EventRate(u64, u32, u64, u32),
    /// Rate-limited differential sampling: (threshold, min_secs, min_nanos, max_secs, max_nanos).
    DifferentialRate(f64, u64, u32, u64, u32),
}

impl SamplingStrategy {
    /// Get the string representation of the sampling strategy.
    pub fn as_str(&self) -> &'static str {
        match self {
            SamplingStrategy::None => "none",
            SamplingStrategy::Auto => "auto",
            SamplingStrategy::Period(_, _) => "period",
            SamplingStrategy::Event => "event",
            SamplingStrategy::Differential(_) => "differential",
            SamplingStrategy::EventRate(_, _, _, _) => "event-rate",
            SamplingStrategy::DifferentialRate(_, _, _, _, _) => "differential-rate",
        }
    }

    /// Parse a sampling strategy from its name and parameters.
    ///
    /// # Arguments
    ///
    /// * `name` - The strategy name (e.g., "event", "period").
    /// * `params` - A slice of string parameters for the strategy.
    ///
    /// # Returns
    ///
    /// `Some(SamplingStrategy)` if the strategy and parameters are valid, or `None` otherwise.
    pub fn parse(name: &str, params: &[&str]) -> Option<Self> {
        match name {
            "none" => Some(SamplingStrategy::None),
            "auto" => Some(SamplingStrategy::Auto),
            "event" => Some(SamplingStrategy::Event),
            "period" => {
                let secs: f64 = params.first()?.parse().ok()?;
                let whole = secs as u64;
                let frac = ((secs - whole as f64) * 1_000_000_000.0) as u32;
                Some(SamplingStrategy::Period(whole, frac))
            }
            "differential" => {
                let diff: f64 = params.first()?.parse().ok()?;
                Some(SamplingStrategy::Differential(diff))
            }
            "event-rate" => {
                let min_secs: f64 = params.first()?.parse().ok()?;
                let min_whole = min_secs as u64;
                let min_frac = ((min_secs - min_whole as f64) * 1_000_000_000.0) as u32;
                let max_secs: f64 = params.get(1)?.parse().ok()?;
                let max_whole = max_secs as u64;
                let max_frac = ((max_secs - max_whole as f64) * 1_000_000_000.0) as u32;
                Some(SamplingStrategy::EventRate(min_whole, min_frac, max_whole, max_frac))
            }
            "differential-rate" => {
                let diff: f64 = params.first()?.parse().ok()?;
                let min_secs: f64 = params.get(1)?.parse().ok()?;
                let min_whole = min_secs as u64;
                let min_frac = ((min_secs - min_whole as f64) * 1_000_000_000.0) as u32;
                let max_secs: f64 = params.get(2)?.parse().ok()?;
                let max_whole = max_secs as u64;
                let max_frac = ((max_secs - max_whole as f64) * 1_000_000_000.0) as u32;
                Some(SamplingStrategy::DifferentialRate(diff, min_whole, min_frac, max_whole, max_frac))
            }
            _ => Option::None,
        }
    }

    /// Get the formatted parameters for this strategy.
    ///
    /// # Returns
    ///
    /// A space-separated string of parameters as used in the `#sensor-status` reply.
    pub fn params_str(&self) -> String {
        match self {
            SamplingStrategy::None | SamplingStrategy::Auto | SamplingStrategy::Event => {
                String::new()
            }
            SamplingStrategy::Period(s, n) => {
                format!("{:.6}", *s as f64 + *n as f64 / 1_000_000_000.0)
            }
            SamplingStrategy::Differential(d) => format!("{}", d),
            SamplingStrategy::EventRate(ms, mn, xs, xn) => {
                format!(
                    "{:.6} {:.6}",
                    *ms as f64 + *mn as f64 / 1_000_000_000.0,
                    *xs as f64 + *xn as f64 / 1_000_000_000.0
                )
            }
            SamplingStrategy::DifferentialRate(d, ms, mn, xs, xn) => {
                format!(
                    "{} {:.6} {:.6}",
                    d,
                    *ms as f64 + *mn as f64 / 1_000_000_000.0,
                    *xs as f64 + *xn as f64 / 1_000_000_000.0
                )
            }
        }
    }
}

/// An active sensor sampling observer that sends inform messages through a channel.
pub struct SamplingObserver {
    sensor_name: String,
    strategy: SamplingStrategy,
    client_addr: SocketAddr,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    state: Mutex<SamplingState>,
}

struct SamplingState {
    last_sent: Option<Instant>,
    last_value: Option<Vec<u8>>,
}

impl SamplingObserver {
    /// Create a new sampling observer.
    ///
    /// # Arguments
    ///
    /// * `sensor_name` - The name of the sensor being observed.
    /// * `strategy` - The sampling strategy to enforce.
    /// * `client_addr` - The address of the client that owns this observer.
    /// * `tx` - The outgoing channel to send generated inform messages to.
    ///
    /// # Returns
    ///
    /// A new `SamplingObserver` wrapped in an `Arc`.
    pub fn new(
        sensor_name: String,
        strategy: SamplingStrategy,
        client_addr: SocketAddr,
        tx: mpsc::UnboundedSender<Vec<u8>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            sensor_name,
            strategy,
            client_addr,
            tx,
            state: Mutex::new(SamplingState {
                last_sent: Option::None,
                last_value: Option::None,
            }),
        })
    }

    /// Determine whether a sensor reading should be sent based on the active strategy.
    fn should_send(&self, reading: &SensorReading) -> bool {
        let mut state = self.state.lock().unwrap();
        match self.strategy {
            SamplingStrategy::None => false,
            SamplingStrategy::Auto | SamplingStrategy::Event => {
                let changed = state.last_value.as_ref() != Some(&reading.value);
                if changed {
                    state.last_value = Some(reading.value.clone());
                    state.last_sent = Some(Instant::now());
                }
                changed
            }
            SamplingStrategy::Period(secs, nanos) => {
                let period = std::time::Duration::new(secs, nanos);
                let now = Instant::now();
                let should = state
                    .last_sent
                    .map(|t| now.duration_since(t) >= period)
                    .unwrap_or(true);
                if should {
                    state.last_sent = Some(now);
                    state.last_value = Some(reading.value.clone());
                }
                should
            }
            SamplingStrategy::Differential(threshold) => {
                let changed = if let Some(ref last) = state.last_value {
                    let last_val: f64 = std::str::from_utf8(last)
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0.0);
                    let cur_val: f64 = std::str::from_utf8(&reading.value)
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0.0);
                    (cur_val - last_val).abs() >= threshold
                } else {
                    true
                };
                if changed {
                    state.last_value = Some(reading.value.clone());
                    state.last_sent = Some(Instant::now());
                }
                changed
            }
            SamplingStrategy::EventRate(min_s, min_n, _max_s, _max_n) => {
                let min_period = std::time::Duration::new(min_s, min_n);
                let now = Instant::now();
                let changed = state.last_value.as_ref() != Some(&reading.value);
                let rate_ok = state
                    .last_sent
                    .map(|t| now.duration_since(t) >= min_period)
                    .unwrap_or(true);
                if changed && rate_ok {
                    state.last_value = Some(reading.value.clone());
                    state.last_sent = Some(now);
                    true
                } else {
                    false
                }
            }
            SamplingStrategy::DifferentialRate(threshold, min_s, min_n, _max_s, _max_n) => {
                let min_period = std::time::Duration::new(min_s, min_n);
                let now = Instant::now();
                let diff_ok = if let Some(ref last) = state.last_value {
                    let last_val: f64 = std::str::from_utf8(last)
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0.0);
                    let cur_val: f64 = std::str::from_utf8(&reading.value)
                        .unwrap_or("0")
                        .parse()
                        .unwrap_or(0.0);
                    (cur_val - last_val).abs() >= threshold
                } else {
                    true
                };
                let rate_ok = state
                    .last_sent
                    .map(|t| now.duration_since(t) >= min_period)
                    .unwrap_or(true);
                if diff_ok && rate_ok {
                    state.last_value = Some(reading.value.clone());
                    state.last_sent = Some(now);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Format and send the sensor reading as a `#sensor-status` inform message.
    fn send_inform(&self, reading: &SensorReading) {
        let inform = Message::inform(
            "sensor-status",
            vec![
                format!("{:.6}", reading.timestamp).into_bytes(),
                b"1".to_vec(), // count
                self.sensor_name.as_bytes().to_vec(),
                reading.status.as_str().as_bytes().to_vec(),
                reading.value.clone(),
            ],
        );
        let _ = self.tx.send(inform.to_bytes());
    }
}

impl SensorObserver for SamplingObserver {
    fn on_update(&self, _sensor_name: &str, reading: &SensorReading) {
        if self.should_send(reading) {
            self.send_inform(reading);
        }
    }

    fn client_addr(&self) -> Option<SocketAddr> {
        Some(self.client_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_strategy() {
        assert_eq!(
            SamplingStrategy::parse("none", &[]),
            Some(SamplingStrategy::None)
        );
        assert_eq!(
            SamplingStrategy::parse("auto", &[]),
            Some(SamplingStrategy::Auto)
        );
        assert_eq!(
            SamplingStrategy::parse("event", &[]),
            Some(SamplingStrategy::Event)
        );

        match SamplingStrategy::parse("period", &["1.5"]) {
            Some(SamplingStrategy::Period(1, n)) => assert!(n > 0),
            other => panic!("expected Period, got {:?}", other),
        }

        assert_eq!(
            SamplingStrategy::parse("differential", &["0.5"]),
            Some(SamplingStrategy::Differential(0.5))
        );
    }

    #[test]
    fn test_strategy_as_str() {
        assert_eq!(SamplingStrategy::None.as_str(), "none");
        assert_eq!(SamplingStrategy::Event.as_str(), "event");
        assert_eq!(SamplingStrategy::Period(1, 0).as_str(), "period");
    }

    #[test]
    fn test_params_str() {
        assert_eq!(SamplingStrategy::None.params_str(), "");
        assert_eq!(SamplingStrategy::Period(1, 0).params_str(), "1.000000");
        assert_eq!(SamplingStrategy::Differential(0.5).params_str(), "0.5");
    }

    fn test_addr() -> std::net::SocketAddr {
        "127.0.0.1:9999".parse().unwrap()
    }

    #[test]
    fn test_sampling_observer_event() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let obs = SamplingObserver::new("test-sensor".into(), SamplingStrategy::Event, test_addr(), tx);

        // First update should always send
        let r1 = SensorReading::new(1.0, crate::sensor::SensorStatus::Nominal, b"10".to_vec());
        obs.on_update("test-sensor", &r1);
        assert!(rx.try_recv().is_ok());

        // Same value should not send
        let r2 = SensorReading::new(2.0, crate::sensor::SensorStatus::Nominal, b"10".to_vec());
        obs.on_update("test-sensor", &r2);
        assert!(rx.try_recv().is_err());

        // Different value should send
        let r3 = SensorReading::new(3.0, crate::sensor::SensorStatus::Nominal, b"20".to_vec());
        obs.on_update("test-sensor", &r3);
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn test_sampling_observer_none() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let obs = SamplingObserver::new("test-sensor".into(), SamplingStrategy::None, test_addr(), tx);

        let r = SensorReading::new(1.0, crate::sensor::SensorStatus::Nominal, b"10".to_vec());
        obs.on_update("test-sensor", &r);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_sampling_observer_differential() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let obs =
            SamplingObserver::new("test-sensor".into(), SamplingStrategy::Differential(5.0), test_addr(), tx);

        let r1 = SensorReading::new(1.0, crate::sensor::SensorStatus::Nominal, b"10".to_vec());
        obs.on_update("test-sensor", &r1);
        assert!(rx.try_recv().is_ok()); // first always sends

        let r2 = SensorReading::new(2.0, crate::sensor::SensorStatus::Nominal, b"12".to_vec());
        obs.on_update("test-sensor", &r2);
        assert!(rx.try_recv().is_err()); // diff < 5

        let r3 = SensorReading::new(3.0, crate::sensor::SensorStatus::Nominal, b"20".to_vec());
        obs.on_update("test-sensor", &r3);
        assert!(rx.try_recv().is_ok()); // diff >= 5
    }
}
