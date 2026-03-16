use std::net::SocketAddr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TypeError {
    #[error("invalid integer: {0}")]
    InvalidInt(String),
    #[error("invalid float: {0}")]
    InvalidFloat(String),
    #[error("invalid boolean: {0}")]
    InvalidBool(String),
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(String),
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    #[error("invalid discrete value {0:?}, expected one of: {1:?}")]
    InvalidDiscrete(String, Vec<String>),
    #[error("value {0} out of range [{1}, {2}]")]
    OutOfRange(String, String, String),
}

/// The type system for KATCP values, mapping between Rust types and wire format.
#[derive(Debug, Clone)]
pub enum KatcpType {
    Int {
        min: Option<i64>,
        max: Option<i64>,
    },
    Float {
        min: Option<f64>,
        max: Option<f64>,
    },
    Bool,
    Str,
    Discrete {
        values: Vec<String>,
    },
    Timestamp,
    Address,
    Lru,
}

/// A typed KATCP value.
#[derive(Debug, Clone, PartialEq)]
pub enum KatcpValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    Timestamp(f64),
    Address(SocketAddr),
    Lru(bool), // true = nominal, false = error
}

impl KatcpType {
    /// Encode a value to KATCP wire format bytes.
    pub fn encode(&self, value: &KatcpValue) -> Result<Vec<u8>, TypeError> {
        match (self, value) {
            (KatcpType::Int { min, max }, KatcpValue::Int(v)) => {
                if let Some(lo) = min {
                    if v < lo {
                        return Err(TypeError::OutOfRange(
                            v.to_string(),
                            lo.to_string(),
                            max.unwrap_or(i64::MAX).to_string(),
                        ));
                    }
                }
                if let Some(hi) = max {
                    if v > hi {
                        return Err(TypeError::OutOfRange(
                            v.to_string(),
                            min.unwrap_or(i64::MIN).to_string(),
                            hi.to_string(),
                        ));
                    }
                }
                Ok(v.to_string().into_bytes())
            }
            (KatcpType::Float { min, max }, KatcpValue::Float(v)) => {
                if let Some(lo) = min {
                    if v < lo {
                        return Err(TypeError::OutOfRange(
                            v.to_string(),
                            lo.to_string(),
                            max.unwrap_or(f64::MAX).to_string(),
                        ));
                    }
                }
                if let Some(hi) = max {
                    if v > hi {
                        return Err(TypeError::OutOfRange(
                            v.to_string(),
                            min.unwrap_or(f64::MIN).to_string(),
                            hi.to_string(),
                        ));
                    }
                }
                Ok(format!("{:.17e}", v).into_bytes())
            }
            (KatcpType::Bool, KatcpValue::Bool(v)) => {
                Ok(if *v { b"1".to_vec() } else { b"0".to_vec() })
            }
            (KatcpType::Str, KatcpValue::Str(v)) => Ok(v.as_bytes().to_vec()),
            (KatcpType::Str, KatcpValue::Bytes(v)) => Ok(v.clone()),
            (KatcpType::Discrete { values }, KatcpValue::Str(v)) => {
                if !values.contains(v) {
                    return Err(TypeError::InvalidDiscrete(v.clone(), values.clone()));
                }
                Ok(v.as_bytes().to_vec())
            }
            (KatcpType::Timestamp, KatcpValue::Timestamp(v)) => {
                Ok(format!("{:.6}", v).into_bytes())
            }
            (KatcpType::Address, KatcpValue::Address(addr)) => {
                Ok(format!("{}:{}", addr.ip(), addr.port()).into_bytes())
            }
            (KatcpType::Lru, KatcpValue::Lru(v)) => {
                Ok(if *v {
                    b"nominal".to_vec()
                } else {
                    b"error".to_vec()
                })
            }
            _ => Err(TypeError::InvalidInt("type mismatch".into())),
        }
    }

    /// Decode KATCP wire format bytes into a typed value.
    pub fn decode(&self, data: &[u8]) -> Result<KatcpValue, TypeError> {
        let s = std::str::from_utf8(data).unwrap_or("");
        match self {
            KatcpType::Int { min, max } => {
                let v: i64 = s.parse().map_err(|_| TypeError::InvalidInt(s.into()))?;
                if let Some(lo) = min {
                    if v < *lo {
                        return Err(TypeError::OutOfRange(
                            v.to_string(),
                            lo.to_string(),
                            max.unwrap_or(i64::MAX).to_string(),
                        ));
                    }
                }
                if let Some(hi) = max {
                    if v > *hi {
                        return Err(TypeError::OutOfRange(
                            v.to_string(),
                            min.unwrap_or(i64::MIN).to_string(),
                            hi.to_string(),
                        ));
                    }
                }
                Ok(KatcpValue::Int(v))
            }
            KatcpType::Float { min, max } => {
                let v: f64 = s.parse().map_err(|_| TypeError::InvalidFloat(s.into()))?;
                if let Some(lo) = min {
                    if v < *lo {
                        return Err(TypeError::OutOfRange(
                            v.to_string(),
                            lo.to_string(),
                            max.unwrap_or(f64::MAX).to_string(),
                        ));
                    }
                }
                if let Some(hi) = max {
                    if v > *hi {
                        return Err(TypeError::OutOfRange(
                            v.to_string(),
                            min.unwrap_or(f64::MIN).to_string(),
                            hi.to_string(),
                        ));
                    }
                }
                Ok(KatcpValue::Float(v))
            }
            KatcpType::Bool => match s {
                "1" => Ok(KatcpValue::Bool(true)),
                "0" => Ok(KatcpValue::Bool(false)),
                _ => Err(TypeError::InvalidBool(s.into())),
            },
            KatcpType::Str => Ok(KatcpValue::Str(s.to_string())),
            KatcpType::Discrete { values } => {
                if !values.contains(&s.to_string()) {
                    return Err(TypeError::InvalidDiscrete(s.into(), values.clone()));
                }
                Ok(KatcpValue::Str(s.to_string()))
            }
            KatcpType::Timestamp => {
                let v: f64 = s
                    .parse()
                    .map_err(|_| TypeError::InvalidTimestamp(s.into()))?;
                Ok(KatcpValue::Timestamp(v))
            }
            KatcpType::Address => {
                let addr: SocketAddr = s
                    .parse()
                    .map_err(|_| TypeError::InvalidAddress(s.into()))?;
                Ok(KatcpValue::Address(addr))
            }
            KatcpType::Lru => match s {
                "nominal" => Ok(KatcpValue::Lru(true)),
                "error" => Ok(KatcpValue::Lru(false)),
                _ => Err(TypeError::InvalidBool(s.into())),
            },
        }
    }
}

/// Encode a timestamp as KATCP wire format.
pub fn encode_timestamp(secs: f64) -> Vec<u8> {
    format!("{:.6}", secs).into_bytes()
}

/// Decode a KATCP timestamp from wire format.
pub fn decode_timestamp(data: &[u8]) -> Result<f64, TypeError> {
    let s = std::str::from_utf8(data).unwrap_or("");
    s.parse()
        .map_err(|_| TypeError::InvalidTimestamp(s.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_int_encode_decode() {
        let t = KatcpType::Int {
            min: None,
            max: None,
        };
        let encoded = t.encode(&KatcpValue::Int(42)).unwrap();
        assert_eq!(encoded, b"42");
        let decoded = t.decode(&encoded).unwrap();
        assert_eq!(decoded, KatcpValue::Int(42));
    }

    #[test]
    fn test_int_range() {
        let t = KatcpType::Int {
            min: Some(0),
            max: Some(100),
        };
        assert!(t.encode(&KatcpValue::Int(50)).is_ok());
        assert!(t.encode(&KatcpValue::Int(-1)).is_err());
        assert!(t.encode(&KatcpValue::Int(101)).is_err());
    }

    #[test]
    fn test_float_encode_decode() {
        let t = KatcpType::Float {
            min: None,
            max: None,
        };
        let encoded = t.encode(&KatcpValue::Float(3.14)).unwrap();
        let decoded = t.decode(&encoded).unwrap();
        match decoded {
            KatcpValue::Float(v) => assert!((v - 3.14).abs() < 1e-10),
            _ => panic!("expected float"),
        }
    }

    #[test]
    fn test_bool_encode_decode() {
        let t = KatcpType::Bool;
        assert_eq!(t.encode(&KatcpValue::Bool(true)).unwrap(), b"1");
        assert_eq!(t.encode(&KatcpValue::Bool(false)).unwrap(), b"0");
        assert_eq!(t.decode(b"1").unwrap(), KatcpValue::Bool(true));
        assert_eq!(t.decode(b"0").unwrap(), KatcpValue::Bool(false));
        assert!(t.decode(b"yes").is_err());
    }

    #[test]
    fn test_str_encode_decode() {
        let t = KatcpType::Str;
        let encoded = t.encode(&KatcpValue::Str("hello".into())).unwrap();
        assert_eq!(encoded, b"hello");
        assert_eq!(
            t.decode(b"hello").unwrap(),
            KatcpValue::Str("hello".into())
        );
    }

    #[test]
    fn test_discrete() {
        let t = KatcpType::Discrete {
            values: vec!["on".into(), "off".into()],
        };
        assert!(t.encode(&KatcpValue::Str("on".into())).is_ok());
        assert!(t.encode(&KatcpValue::Str("maybe".into())).is_err());
    }

    #[test]
    fn test_timestamp() {
        let t = KatcpType::Timestamp;
        let encoded = t.encode(&KatcpValue::Timestamp(1234567890.123456)).unwrap();
        let s = std::str::from_utf8(&encoded).unwrap();
        assert!(s.contains("1234567890.123456"));
    }

    #[test]
    fn test_lru() {
        let t = KatcpType::Lru;
        assert_eq!(t.encode(&KatcpValue::Lru(true)).unwrap(), b"nominal");
        assert_eq!(t.encode(&KatcpValue::Lru(false)).unwrap(), b"error");
        assert_eq!(t.decode(b"nominal").unwrap(), KatcpValue::Lru(true));
        assert_eq!(t.decode(b"error").unwrap(), KatcpValue::Lru(false));
    }
}
