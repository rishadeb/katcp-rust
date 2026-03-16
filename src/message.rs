use std::fmt;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MessageError {
    #[error("empty message")]
    Empty,
    #[error("invalid message type character: {0:?}")]
    InvalidType(char),
    #[error("invalid message name: {0:?}")]
    InvalidName(String),
    #[error("invalid message ID: {0:?}")]
    InvalidMid(String),
    #[error("message too large: {0} bytes")]
    TooLarge(usize),
    #[error("invalid escape sequence: \\{0}")]
    InvalidEscape(char),
}

/// Maximum message size (2 MB).
pub const MAX_MSG_SIZE: usize = 2 * 1024 * 1024;

/// KATCP message types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageType {
    Request,
    Reply,
    Inform,
}

impl MessageType {
    /// Get the prefix character for this message type ('?', '!', or '#').
    pub fn prefix(&self) -> char {
        match self {
            MessageType::Request => '?',
            MessageType::Reply => '!',
            MessageType::Inform => '#',
        }
    }

    /// Parse a `MessageType` from its prefix character.
    ///
    /// # Arguments
    ///
    /// * `c` - The prefix character ('?', '!', or '#').
    ///
    /// # Returns
    ///
    /// Returns the corresponding `MessageType`.
    ///
    /// # Errors
    ///
    /// Returns `MessageError::InvalidType` if the character does not match a known prefix.
    pub fn from_char(c: char) -> Result<Self, MessageError> {
        match c {
            '?' => Ok(MessageType::Request),
            '!' => Ok(MessageType::Reply),
            '#' => Ok(MessageType::Inform),
            _ => Err(MessageError::InvalidType(c)),
        }
    }
}

/// A single KATCP protocol message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub mtype: MessageType,
    pub name: String,
    pub mid: Option<String>,
    pub arguments: Vec<Vec<u8>>,
}

impl Message {
    /// Create a new request message.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the request message.
    /// * `args` - A vector of byte vectors representing the message arguments.
    ///
    /// # Returns
    ///
    /// A new `Message` configured as a request.
    pub fn request(name: &str, args: Vec<Vec<u8>>) -> Self {
        Self {
            mtype: MessageType::Request,
            name: name.to_string(),
            mid: None,
            arguments: args,
        }
    }

    /// Create a new reply message.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the reply message.
    /// * `args` - A vector of byte vectors representing the message arguments.
    ///
    /// # Returns
    ///
    /// A new `Message` configured as a reply.
    pub fn reply(name: &str, args: Vec<Vec<u8>>) -> Self {
        Self {
            mtype: MessageType::Reply,
            name: name.to_string(),
            mid: None,
            arguments: args,
        }
    }

    /// Create a new inform message.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the inform message.
    /// * `args` - A vector of byte vectors representing the message arguments.
    ///
    /// # Returns
    ///
    /// A new `Message` configured as an inform.
    pub fn inform(name: &str, args: Vec<Vec<u8>>) -> Self {
        Self {
            mtype: MessageType::Inform,
            name: name.to_string(),
            mid: None,
            arguments: args,
        }
    }

    /// Create a reply to a given request, preserving the message ID.
    ///
    /// # Arguments
    ///
    /// * `req` - The originating request `Message`.
    /// * `args` - The arguments to include in the reply.
    ///
    /// # Returns
    ///
    /// A new reply `Message` with the exact name and message ID as the request.
    pub fn reply_to_request(req: &Message, args: Vec<Vec<u8>>) -> Self {
        Self {
            mtype: MessageType::Reply,
            name: req.name.clone(),
            mid: req.mid.clone(),
            arguments: args,
        }
    }

    /// Create an inform in response to a given request, preserving the message ID.
    pub fn reply_inform(req: &Message, args: Vec<Vec<u8>>) -> Self {
        Self {
            mtype: MessageType::Inform,
            name: req.name.clone(),
            mid: req.mid.clone(),
            arguments: args,
        }
    }

    /// Set the message ID.
    pub fn with_mid(mut self, mid: &str) -> Self {
        self.mid = Some(mid.to_string());
        self
    }

    /// Convenience: create a request with string arguments.
    pub fn request_with_str_args(name: &str, args: &[&str]) -> Self {
        Self::request(
            name,
            args.iter().map(|s| s.as_bytes().to_vec()).collect(),
        )
    }

    /// Convenience: create a reply with string arguments.
    pub fn reply_with_str_args(name: &str, args: &[&str]) -> Self {
        Self::reply(
            name,
            args.iter().map(|s| s.as_bytes().to_vec()).collect(),
        )
    }

    /// Convenience: create an inform with string arguments.
    pub fn inform_with_str_args(name: &str, args: &[&str]) -> Self {
        Self::inform(
            name,
            args.iter().map(|s| s.as_bytes().to_vec()).collect(),
        )
    }

    /// Get the first argument as a UTF-8 string (typically the status for replies).
    ///
    /// # Returns
    ///
    /// `Some(&str)` if the first argument exists and is valid UTF-8, `None` otherwise.
    pub fn reply_status(&self) -> Option<&str> {
        self.arguments.first().and_then(|a| std::str::from_utf8(a).ok())
    }

    /// Check if this is an "ok" reply.
    pub fn is_ok(&self) -> bool {
        self.reply_status() == Some("ok")
    }

    /// Serialize the message to its wire format bytes.
    ///
    /// # Returns
    ///
    /// A generic byte vector containing the fully serialized, escaped KATCP message terminated with a newline.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);
        buf.push(self.mtype.prefix() as u8);
        buf.extend_from_slice(self.name.as_bytes());
        if let Some(ref mid) = self.mid {
            buf.push(b'[');
            buf.extend_from_slice(mid.as_bytes());
            buf.push(b']');
        }
        for arg in &self.arguments {
            buf.push(b' ');
            if arg.is_empty() {
                buf.extend_from_slice(b"\\@");
            } else {
                escape_arg_into(&mut buf, arg);
            }
        }
        buf.push(b'\n');
        buf
    }

    /// Get an argument as a UTF-8 string.
    ///
    /// # Arguments
    ///
    /// * `index` - The zero-based index of the argument.
    ///
    /// # Returns
    ///
    /// `Some(&str)` if the argument exists and is valid UTF-8, `None` otherwise.
    pub fn arg_str(&self, index: usize) -> Option<&str> {
        self.arguments.get(index).and_then(|a| std::str::from_utf8(a).ok())
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.to_bytes();
        // Strip trailing newline for display
        let s = std::str::from_utf8(&bytes[..bytes.len() - 1]).unwrap_or("<binary>");
        write!(f, "{}", s)
    }
}

/// Escape a single argument for the KATCP wire format.
fn escape_arg_into(buf: &mut Vec<u8>, data: &[u8]) {
    for &b in data {
        match b {
            b'\\' => buf.extend_from_slice(b"\\\\"),
            b' ' => buf.extend_from_slice(b"\\_"),
            b'\0' => buf.extend_from_slice(b"\\0"),
            b'\n' => buf.extend_from_slice(b"\\n"),
            b'\r' => buf.extend_from_slice(b"\\r"),
            0x1b => buf.extend_from_slice(b"\\e"),
            b'\t' => buf.extend_from_slice(b"\\t"),
            _ => buf.push(b),
        }
    }
}

/// Unescape a KATCP argument from wire format.
fn unescape_arg(data: &[u8]) -> Result<Vec<u8>, MessageError> {
    if data == b"\\@" {
        return Ok(Vec::new());
    }
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\\' {
            i += 1;
            if i >= data.len() {
                return Err(MessageError::InvalidEscape(' '));
            }
            match data[i] {
                b'\\' => result.push(b'\\'),
                b'_' => result.push(b' '),
                b'0' => result.push(b'\0'),
                b'n' => result.push(b'\n'),
                b'r' => result.push(b'\r'),
                b'e' => result.push(0x1b),
                b't' => result.push(b'\t'),
                b'@' => {} // empty string marker in middle of data - unusual but handle it
                other => return Err(MessageError::InvalidEscape(other as char)),
            }
        } else {
            result.push(data[i]);
        }
        i += 1;
    }
    Ok(result)
}

/// Parses raw bytes into KATCP messages.
pub struct MessageParser;

impl MessageParser {
    /// Parse a single line (without trailing newline) into a Message.
    ///
    /// # Arguments
    ///
    /// * `line` - The raw byte slice of a KATCP line, excluding the newline character.
    ///
    /// # Returns
    ///
    /// Returns the parsed `Message`.
    ///
    /// # Errors
    ///
    /// Returns a `MessageError` if the line is empty, too large, has invalid framing/types, 
    /// or contains invalid escapes.
    pub fn parse(line: &[u8]) -> Result<Message, MessageError> {
        if line.is_empty() {
            return Err(MessageError::Empty);
        }
        if line.len() > MAX_MSG_SIZE {
            return Err(MessageError::TooLarge(line.len()));
        }

        let mtype = MessageType::from_char(line[0] as char)?;

        // Find end of name (space, tab, or end of line)
        let rest = &line[1..];
        let name_end = rest
            .iter()
            .position(|&b| b == b' ' || b == b'\t' || b == b'[')
            .unwrap_or(rest.len());

        let name_bytes = &rest[..name_end];
        if name_bytes.is_empty() {
            return Err(MessageError::InvalidName(String::new()));
        }

        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| MessageError::InvalidName(String::from_utf8_lossy(name_bytes).into()))?;

        // Validate name: must start with a letter, contain only alphanumeric and dashes
        if !name.starts_with(|c: char| c.is_ascii_alphabetic())
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(MessageError::InvalidName(name.to_string()));
        }

        let mut pos = 1 + name_end;

        // Parse optional message ID [digits]
        let mid = if pos < line.len() && line[pos] == b'[' {
            let bracket_start = pos + 1;
            let bracket_end = line[bracket_start..]
                .iter()
                .position(|&b| b == b']')
                .map(|p| bracket_start + p)
                .ok_or_else(|| MessageError::InvalidMid("unclosed bracket".into()))?;
            let mid_bytes = &line[bracket_start..bracket_end];
            let mid_str = std::str::from_utf8(mid_bytes)
                .map_err(|_| MessageError::InvalidMid(String::from_utf8_lossy(mid_bytes).into()))?;
            if !mid_str.chars().all(|c| c.is_ascii_digit()) {
                return Err(MessageError::InvalidMid(mid_str.to_string()));
            }
            pos = bracket_end + 1;
            Some(mid_str.to_string())
        } else {
            None
        };

        // Parse arguments (separated by spaces/tabs)
        let mut arguments = Vec::new();
        let arg_data = if pos < line.len() { &line[pos..] } else { &[] };

        // Split on whitespace, skipping leading whitespace
        let mut i = 0;
        while i < arg_data.len() {
            // Skip whitespace
            if arg_data[i] == b' ' || arg_data[i] == b'\t' {
                i += 1;
                continue;
            }
            // Find end of this argument
            let arg_start = i;
            while i < arg_data.len() && arg_data[i] != b' ' && arg_data[i] != b'\t' {
                i += 1;
            }
            let raw_arg = &arg_data[arg_start..i];
            arguments.push(unescape_arg(raw_arg)?);
        }

        Ok(Message {
            mtype,
            name: name.to_string(),
            mid,
            arguments,
        })
    }

    /// Split a byte stream into lines and parse each one.
    /// Returns parsed messages and any remaining incomplete data.
    pub fn parse_stream(data: &[u8]) -> (Vec<Result<Message, MessageError>>, Vec<u8>) {
        let mut messages = Vec::new();
        let mut start = 0;

        for (i, &b) in data.iter().enumerate() {
            if b == b'\n' {
                let line = &data[start..i];
                // Strip trailing \r if present
                let line = if line.last() == Some(&b'\r') {
                    &line[..line.len() - 1]
                } else {
                    line
                };
                if !line.is_empty() {
                    messages.push(Self::parse(line));
                }
                start = i + 1;
            }
        }

        let remainder = data[start..].to_vec();
        (messages, remainder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_request() {
        let msg = MessageParser::parse(b"?help").unwrap();
        assert_eq!(msg.mtype, MessageType::Request);
        assert_eq!(msg.name, "help");
        assert!(msg.arguments.is_empty());
        assert!(msg.mid.is_none());
    }

    #[test]
    fn test_parse_request_with_args() {
        let msg = MessageParser::parse(b"?sensor-value temperature").unwrap();
        assert_eq!(msg.mtype, MessageType::Request);
        assert_eq!(msg.name, "sensor-value");
        assert_eq!(msg.arguments, vec![b"temperature".to_vec()]);
    }

    #[test]
    fn test_parse_reply() {
        let msg = MessageParser::parse(b"!help ok 3").unwrap();
        assert_eq!(msg.mtype, MessageType::Reply);
        assert_eq!(msg.name, "help");
        assert_eq!(msg.arguments, vec![b"ok".to_vec(), b"3".to_vec()]);
    }

    #[test]
    fn test_parse_inform() {
        let msg = MessageParser::parse(b"#version-connect katcp-protocol 5.0-MI").unwrap();
        assert_eq!(msg.mtype, MessageType::Inform);
        assert_eq!(msg.name, "version-connect");
        assert_eq!(
            msg.arguments,
            vec![b"katcp-protocol".to_vec(), b"5.0-MI".to_vec()]
        );
    }

    #[test]
    fn test_parse_with_mid() {
        let msg = MessageParser::parse(b"?help[123]").unwrap();
        assert_eq!(msg.mtype, MessageType::Request);
        assert_eq!(msg.name, "help");
        assert_eq!(msg.mid, Some("123".to_string()));
    }

    #[test]
    fn test_parse_with_mid_and_args() {
        let msg = MessageParser::parse(b"!add[42] ok 7.5").unwrap();
        assert_eq!(msg.mtype, MessageType::Reply);
        assert_eq!(msg.name, "add");
        assert_eq!(msg.mid, Some("42".to_string()));
        assert_eq!(msg.arguments, vec![b"ok".to_vec(), b"7.5".to_vec()]);
    }

    #[test]
    fn test_escape_unescape_roundtrip() {
        let original = b"hello world\ntest\\path\ttab\0null";
        let msg = Message::request("test", vec![original.to_vec()]);
        let bytes = msg.to_bytes();
        let parsed = MessageParser::parse(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(parsed.arguments[0], original.to_vec());
    }

    #[test]
    fn test_empty_argument() {
        let msg = Message::request("test", vec![Vec::new(), b"hello".to_vec()]);
        let bytes = msg.to_bytes();
        let parsed = MessageParser::parse(&bytes[..bytes.len() - 1]).unwrap();
        assert_eq!(parsed.arguments[0], Vec::new());
        assert_eq!(parsed.arguments[1], b"hello".to_vec());
    }

    #[test]
    fn test_serialize_request() {
        let msg = Message::request_with_str_args("help", &[]);
        assert_eq!(msg.to_bytes(), b"?help\n");
    }

    #[test]
    fn test_serialize_reply_with_mid() {
        let msg = Message::reply_with_str_args("help", &["ok", "3"]).with_mid("1");
        assert_eq!(msg.to_bytes(), b"!help[1] ok 3\n");
    }

    #[test]
    fn test_reply_to_request() {
        let req = Message::request_with_str_args("add", &["3", "4"]).with_mid("7");
        let reply = Message::reply_to_request(&req, vec![b"ok".to_vec(), b"7".to_vec()]);
        assert_eq!(reply.mtype, MessageType::Reply);
        assert_eq!(reply.name, "add");
        assert_eq!(reply.mid, Some("7".to_string()));
    }

    #[test]
    fn test_parse_stream() {
        let data = b"?help\n!help ok\n#version katcp";
        let (msgs, remainder) = MessageParser::parse_stream(data);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].is_ok());
        assert!(msgs[1].is_ok());
        assert_eq!(remainder, b"#version katcp");
    }

    #[test]
    fn test_invalid_type() {
        assert!(MessageParser::parse(b"xhelp").is_err());
    }

    #[test]
    fn test_invalid_name() {
        assert!(MessageParser::parse(b"?123bad").is_err());
    }

    #[test]
    fn test_display() {
        let msg = Message::request_with_str_args("help", &[]);
        assert_eq!(format!("{}", msg), "?help");
    }

    #[test]
    fn test_is_ok() {
        let msg = Message::reply_with_str_args("test", &["ok", "value"]);
        assert!(msg.is_ok());

        let msg = Message::reply_with_str_args("test", &["fail", "error"]);
        assert!(!msg.is_ok());
    }
}
