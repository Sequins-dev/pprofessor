use std::fmt;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const STREAM_HEADER_LEN: usize = 28;
const MAGIC: &[u8; 4] = b"PPRS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum FrameKind {
    Hello = 1,
    ProfileCheckpoint = 2,
    ProfileDelta = 3,
    Finalizing = 4,
    FinalProfile = 5,
    Failed = 6,
    Heartbeat = 7,
    Acknowledged = 101,
    Stop = 102,
    ProtocolError = 103,
}

impl TryFrom<u16> for FrameKind {
    type Error = StreamProtocolError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::ProfileCheckpoint),
            3 => Ok(Self::ProfileDelta),
            4 => Ok(Self::Finalizing),
            5 => Ok(Self::FinalProfile),
            6 => Ok(Self::Failed),
            7 => Ok(Self::Heartbeat),
            101 => Ok(Self::Acknowledged),
            102 => Ok(Self::Stop),
            103 => Ok(Self::ProtocolError),
            _ => Err(StreamProtocolError("unknown frame kind")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamHeader {
    pub major: u16,
    pub minor: u16,
    pub kind: FrameKind,
    pub flags: u16,
    pub sequence: u64,
    pub payload_len: u64,
}

impl StreamHeader {
    pub fn encode(&self) -> [u8; STREAM_HEADER_LEN] {
        let mut bytes = [0u8; STREAM_HEADER_LEN];
        bytes[0..4].copy_from_slice(MAGIC);
        bytes[4..6].copy_from_slice(&self.major.to_be_bytes());
        bytes[6..8].copy_from_slice(&self.minor.to_be_bytes());
        bytes[8..10].copy_from_slice(&(self.kind as u16).to_be_bytes());
        bytes[10..12].copy_from_slice(&self.flags.to_be_bytes());
        bytes[12..20].copy_from_slice(&self.sequence.to_be_bytes());
        bytes[20..28].copy_from_slice(&self.payload_len.to_be_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StreamProtocolError> {
        if bytes.len() != STREAM_HEADER_LEN {
            return Err(StreamProtocolError("invalid header length"));
        }
        if &bytes[0..4] != MAGIC {
            return Err(StreamProtocolError("invalid stream magic"));
        }
        Ok(Self {
            major: u16::from_be_bytes(bytes[4..6].try_into().unwrap()),
            minor: u16::from_be_bytes(bytes[6..8].try_into().unwrap()),
            kind: FrameKind::try_from(u16::from_be_bytes(bytes[8..10].try_into().unwrap()))?,
            flags: u16::from_be_bytes(bytes[10..12].try_into().unwrap()),
            sequence: u64::from_be_bytes(bytes[12..20].try_into().unwrap()),
            payload_len: u64::from_be_bytes(bytes[20..28].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamProtocolError(&'static str);

impl fmt::Display for StreamProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for StreamProtocolError {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionHello {
    pub session_id: String,
    pub mode: String,
    pub pid: u32,
    pub process_name: String,
    pub frequency_hz: u32,
}

impl SessionHello {
    pub fn new(
        session_id: impl Into<String>,
        mode: impl Into<String>,
        pid: u32,
        process_name: impl Into<String>,
        frequency_hz: u32,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            mode: mode.into(),
            pid,
            process_name: process_name.into(),
            frequency_hz,
        }
    }
}

pub struct SessionPublisher {
    socket_path: PathBuf,
    hello: SessionHello,
    stream: Option<UnixStream>,
    sequence: u64,
    just_connected: bool,
}

impl SessionPublisher {
    pub fn new(socket_path: PathBuf, hello: SessionHello) -> Self {
        Self {
            socket_path,
            hello,
            stream: None,
            sequence: 0,
            just_connected: false,
        }
    }

    pub fn default_socket_path() -> PathBuf {
        std::env::temp_dir().join("pprofessor-v1.sock")
    }

    pub fn take_just_connected(&mut self) -> bool {
        std::mem::take(&mut self.just_connected)
    }

    pub fn ensure_connected(&mut self) -> std::io::Result<bool> {
        if self.stream.is_some() {
            Ok(true)
        } else {
            self.connect()
        }
    }

    pub fn send(&mut self, kind: FrameKind, payload: &[u8]) -> std::io::Result<bool> {
        if self.stream.is_none() && !self.connect()? {
            return Ok(false);
        }
        self.sequence = self.sequence.saturating_add(1);
        let result = Self::write_frame(self.stream.as_mut().unwrap(), kind, self.sequence, payload);
        if let Err(error) = result {
            self.stream = None;
            return if matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WouldBlock
            ) {
                Ok(false)
            } else {
                Err(error)
            };
        }
        Ok(true)
    }

    fn connect(&mut self) -> std::io::Result<bool> {
        let mut stream = match UnixStream::connect(&self.socket_path) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        stream.set_write_timeout(Some(Duration::from_millis(100)))?;
        let hello = serde_json::to_vec(&self.hello).map_err(std::io::Error::other)?;
        self.sequence = self.sequence.saturating_add(1);
        Self::write_frame(&mut stream, FrameKind::Hello, self.sequence, &hello)?;
        self.stream = Some(stream);
        self.just_connected = true;
        Ok(true)
    }

    fn write_frame(
        stream: &mut UnixStream,
        kind: FrameKind,
        sequence: u64,
        payload: &[u8],
    ) -> std::io::Result<()> {
        let header = StreamHeader {
            major: 1,
            minor: 0,
            kind,
            flags: 0,
            sequence,
            payload_len: payload.len() as u64,
        };
        stream.write_all(&header.encode())?;
        stream.write_all(payload)?;
        Ok(())
    }
}
