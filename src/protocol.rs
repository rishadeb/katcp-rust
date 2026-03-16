use std::collections::HashSet;
use std::fmt;

/// Protocol flags for KATCP v5+.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtocolFlag {
    /// Server supports multiple simultaneous clients.
    MultiClient,
    /// Server supports message identifiers.
    MessageIds,
    /// Server provides request timeout hints.
    RequestTimeoutHints,
    /// Server supports bulk sensor sampling.
    BulkSetSensorSampling,
}

impl ProtocolFlag {
    /// Get the character representation of the protocol flag.
    pub fn to_char(&self) -> char {
        match self {
            ProtocolFlag::MultiClient => 'M',
            ProtocolFlag::MessageIds => 'I',
            ProtocolFlag::RequestTimeoutHints => 'T',
            ProtocolFlag::BulkSetSensorSampling => 'B',
        }
    }

    /// Parse a protocol flag from its character representation.
    ///
    /// # Arguments
    ///
    /// * `c` - The character to parse ('M', 'I', 'T', or 'B').
    ///
    /// # Returns
    ///
    /// `Some(ProtocolFlag)` if the character is recognized, or `None` otherwise.
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            'M' => Some(ProtocolFlag::MultiClient),
            'I' => Some(ProtocolFlag::MessageIds),
            'T' => Some(ProtocolFlag::RequestTimeoutHints),
            'B' => Some(ProtocolFlag::BulkSetSensorSampling),
            _ => None,
        }
    }
}

/// KATCP protocol version and flags.
#[derive(Debug, Clone)]
pub struct ProtocolFlags {
    pub major: u32,
    pub minor: u32,
    pub flags: HashSet<ProtocolFlag>,
}

impl ProtocolFlags {
    /// Create a new `ProtocolFlags` instance with the specified major and minor versions and flags.
    ///
    /// # Arguments
    ///
    /// * `major` - The major version number (e.g., 5).
    /// * `minor` - The minor version number (e.g., 0).
    /// * `flags` - A `HashSet` of `ProtocolFlag`s supported by this version.
    ///
    /// # Returns
    ///
    /// A new `ProtocolFlags` instance.
    pub fn new(major: u32, minor: u32, flags: HashSet<ProtocolFlag>) -> Self {
        Self {
            major,
            minor,
            flags,
        }
    }

    /// Default KATCP v5 with multi-client and message IDs.
    pub fn default_v5() -> Self {
        let mut flags = HashSet::new();
        flags.insert(ProtocolFlag::MultiClient);
        flags.insert(ProtocolFlag::MessageIds);
        Self {
            major: 5,
            minor: 0,
            flags,
        }
    }

    /// Minimal KATCP v5 with no optional flags.
    pub fn minimal_v5() -> Self {
        Self {
            major: 5,
            minor: 0,
            flags: HashSet::new(),
        }
    }

    /// Check if the specified protocol flag is supported.
    ///
    /// # Arguments
    ///
    /// * `flag` - The `ProtocolFlag` to check.
    ///
    /// # Returns
    ///
    /// `true` if the flag is supported, `false` otherwise.
    pub fn supports(&self, flag: ProtocolFlag) -> bool {
        self.flags.contains(&flag)
    }

    /// Check if the MessageIds flag is supported.
    pub fn has_message_ids(&self) -> bool {
        self.supports(ProtocolFlag::MessageIds)
    }

    /// Check if the MultiClient flag is supported.
    pub fn has_multi_client(&self) -> bool {
        self.supports(ProtocolFlag::MultiClient)
    }

    /// Parse a version string like "5.0-MI" into ProtocolFlags.
    ///
    /// # Arguments
    ///
    /// * `version` - The version string to parse.
    ///
    /// # Returns
    ///
    /// `Some(ProtocolFlags)` if the string is valid, or `None` if the format is invalid.
    pub fn parse(version: &str) -> Option<Self> {
        let (version_part, flags_part) = if let Some(idx) = version.find('-') {
            (&version[..idx], Some(&version[idx + 1..]))
        } else {
            (version, None)
        };

        let mut parts = version_part.split('.');
        let major: u32 = parts.next()?.parse().ok()?;
        let minor: u32 = parts.next().unwrap_or("0").parse().ok()?;

        let mut flags = HashSet::new();
        if let Some(flag_str) = flags_part {
            for c in flag_str.chars() {
                if let Some(flag) = ProtocolFlag::from_char(c) {
                    flags.insert(flag);
                }
            }
        }

        Some(Self {
            major,
            minor,
            flags,
        })
    }
}

impl fmt::Display for ProtocolFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if !self.flags.is_empty() {
            write!(f, "-")?;
            // Sort flags for deterministic output
            let mut chars: Vec<char> = self.flags.iter().map(|f| f.to_char()).collect();
            chars.sort();
            for c in chars {
                write!(f, "{}", c)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        let pf = ProtocolFlags::parse("5.0-MI").unwrap();
        assert_eq!(pf.major, 5);
        assert_eq!(pf.minor, 0);
        assert!(pf.has_message_ids());
        assert!(pf.has_multi_client());
    }

    #[test]
    fn test_parse_version_no_flags() {
        let pf = ProtocolFlags::parse("5.0").unwrap();
        assert_eq!(pf.major, 5);
        assert_eq!(pf.minor, 0);
        assert!(!pf.has_message_ids());
    }

    #[test]
    fn test_display() {
        let pf = ProtocolFlags::default_v5();
        let s = pf.to_string();
        assert!(s.starts_with("5.0-"));
        assert!(s.contains('M'));
        assert!(s.contains('I'));
    }

    #[test]
    fn test_parse_roundtrip() {
        let pf = ProtocolFlags::default_v5();
        let s = pf.to_string();
        let pf2 = ProtocolFlags::parse(&s).unwrap();
        assert_eq!(pf2.major, pf.major);
        assert_eq!(pf2.minor, pf.minor);
        assert_eq!(pf2.flags, pf.flags);
    }
}
