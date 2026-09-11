#![forbid(unsafe_code)]

//! Streams that arrive over DDS. One sample is one Stream: a `DATA`
//! submessage where it fits a datagram, else a run of `DATA_FRAG`
//! submessages put back together by sequence number at the reader.
//!
//! DDS is the data bus of the vehicle, the robot and the control room:
//! writers publish samples to topics, readers take them, and RTPS is the
//! wire beneath — messages of submessages over UDP, unicast to a locator or
//! multicast for discovery. What is here is the RTPS message, a sample as
//! a CDR octet sequence, the fragmentation, and the reader's reassembly. A
//! Send Location is a writer addressing a reader's unicast locator; a
//! Receive Location is a reader on one. Discovery is not here: the locator
//! is given, as a Location names it.
//!
//! The datagram is udp's ([`udp::UdpTransport`]); the far end is
//! in-process: [`DdsTransport::receive`] is a reader on a UDP socket, and
//! the loopback pair is a writer and that reader on this machine. The
//! origin URI names the writer: `dds://127.0.0.1:49152/<guid>?sn=1`.

pub mod rtps;

use std::net::UdpSocket;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

pub use rtps::{Message, Reassembly, Submessage};
use transport::error::{Result, protocol_error};
use transport::loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback};
use transport::{Arrived, Directions, Transport};
use udp::UdpTransport;

/// A writer at one locator, or a reader on one.
#[derive(Clone)]
pub struct DdsTransport {
    bind: String,
    locator: String,
    guid_prefix: [u8; 12],
    sequence: Arc<Mutex<i64>>,
    timeout: Duration,
}

impl DdsTransport {
    /// Bound at `bind`, writing to the reader at `locator`, with a GUID
    /// prefix drawn from the process and the bind address.
    #[must_use]
    pub fn new(bind: impl Into<String>, locator: impl Into<String>) -> Self {
        let bind = bind.into();
        let mut guid_prefix = [0u8; 12];
        guid_prefix[..4].copy_from_slice(&std::process::id().to_le_bytes());
        for (at, byte) in bind.bytes().enumerate() {
            guid_prefix[4 + at % 8] ^= byte;
        }
        Self {
            bind,
            locator: locator.into(),
            guid_prefix,
            sequence: Arc::new(Mutex::new(1)),
            timeout: Duration::from_secs(5),
        }
    }

    /// Give up on a sample whose fragments stop coming for `timeout`.
    #[must_use]
    pub const fn timing_out_after(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn datagrams(&self) -> UdpTransport {
        UdpTransport::new(self.bind.clone()).timing_out_after(self.timeout)
    }

    /// Bind the reader's socket and report the locator actually assigned.
    ///
    /// # Errors
    /// Where the address is taken, malformed, or not permitted.
    pub fn bind(&self) -> Result<(UdpSocket, String)> {
        self.datagrams().bind()
    }

    fn next_sequence(&self) -> i64 {
        let mut sequence = self.sequence.lock().unwrap_or_else(PoisonError::into_inner);
        let next = *sequence;
        *sequence += 1;
        next
    }

    /// Write `bytes` as one sample to the reader at `locator`.
    ///
    /// # Errors
    /// Where a datagram could not be sent.
    pub fn write(&self, locator: &str, bytes: &[u8]) -> Result<()> {
        let sequence = self.next_sequence();
        let serialized = rtps::serialize(bytes);
        let datagrams = self.datagrams();
        for message in rtps::messages(
            self.guid_prefix,
            rtps::WRITER_WITH_KEY,
            sequence,
            &serialized,
        ) {
            datagrams.send(locator, &message.encode())?;
        }
        Ok(())
    }

    /// Take one sample on an already-bound socket, or `None` when nothing
    /// arrived in time.
    ///
    /// # Errors
    /// Where a datagram could not be read, a message does not read, or the
    /// fragments stopped coming.
    pub fn read_one(&self, socket: &UdpSocket) -> Result<Option<Arrived>> {
        let datagrams = self.datagrams();
        let mut reassembly = Reassembly::default();
        let mut first = true;
        loop {
            let datagram = match datagrams.receive_one(socket) {
                Ok(datagram) => datagram,
                Err(error) if error.retryable && first => return Ok(None),
                Err(error) => return Err(error),
            };
            first = false;
            let message = Message::decode(&datagram.bytes)?;
            for submessage in &message.submessages {
                if let Some(serialized) = reassembly.take(submessage)? {
                    let (writer_id, sequence) = match submessage {
                        Submessage::Data {
                            writer_id,
                            sequence,
                            ..
                        }
                        | Submessage::DataFrag {
                            writer_id,
                            sequence,
                            ..
                        } => (writer_id, sequence),
                        Submessage::InfoTimestamp { .. } => continue,
                    };
                    let peer = datagram.origin_uri.trim_start_matches("udp://");
                    let origin = format!(
                        "dds://{peer}/{}{}?sn={sequence}",
                        hex(&message.guid_prefix),
                        hex(writer_id)
                    );
                    return Ok(Some(Arrived::new(origin, rtps::deserialize(&serialized)?)));
                }
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

impl Transport for DdsTransport {
    fn name(&self) -> &'static str {
        "dds"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    /// No writer writing is not an error: an empty vector.
    fn receive(&self) -> Result<Vec<Arrived>> {
        let (socket, _) = self.bind()?;
        Ok(self.read_one(&socket)?.into_iter().collect())
    }

    /// `target` may name the reader's locator, `dds://host:7411`, overriding
    /// the transport's.
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        match transport::socket::target("dds", target) {
            Some((locator, _)) if !locator.is_empty() => self.write(locator, bytes),
            _ => self.write(&self.locator, bytes),
        }
    }
}

impl DdsTransport {
    /// Both ends on this machine: an ephemeral local port each, the
    /// loopback timeout on the reader.
    #[must_use]
    pub fn loopback() -> Self {
        Self::new("127.0.0.1:0", "127.0.0.1:0").timing_out_after(LOOPBACK_TIMEOUT)
    }
}

/// A reader bound at its locator, waiting for its one sample.
struct Reader {
    transport: DdsTransport,
    socket: UdpSocket,
    locator: String,
}

impl FarEnd for Reader {
    fn address(&self) -> &str {
        &self.locator
    }

    fn take_one(self: Box<Self>) -> Result<Arrived> {
        self.transport
            .read_one(&self.socket)?
            .ok_or_else(|| protocol_error("no sample arrived"))
    }
}

impl Loopback for DdsTransport {
    fn far_end(&self) -> Result<Box<dyn FarEnd>> {
        let (socket, locator) = self.bind()?;
        Ok(Box::new(Reader {
            transport: self.clone(),
            socket,
            locator,
        }))
    }

    fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
        Self::new("127.0.0.1:0", address)
            .timing_out_after(self.timeout)
            .send("", payload)
    }

    fn unblock(&self, _address: &str) {
        // The reader's receive has its own timeout; there is no listener to
        // poke.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shapes a protocol breaks on, as the Playground lists them.
    fn edge_payloads() -> Vec<(&'static str, Vec<u8>)> {
        let patterned = |len: usize| -> Vec<u8> {
            (0..len)
                .map(|at| u8::try_from((at * 31 + at / 251) % 256).unwrap_or(0))
                .collect()
        };
        vec![
            ("empty", Vec::new()),
            ("one byte", vec![0x2a]),
            ("every byte", (0..=255).collect()),
            ("nul run", vec![0; 512]),
            ("high bytes", vec![0xff; 512]),
            ("crlf storm", b"\r\n".repeat(400)),
            ("udp maximum", patterned(65_507)),
            ("sixteen bits plus one", patterned(65_537)),
            ("a mebibyte", patterned(1 << 20)),
        ]
    }

    #[test]
    fn a_loopback_round_writes_a_sample_the_reader_takes() {
        let loopback = DdsTransport::loopback();
        let arrived = loopback.round(b"{\"speed\": 12.5}").expect("round");
        assert_eq!(arrived.bytes, b"{\"speed\": 12.5}");
        assert!(
            arrived.origin_uri.starts_with("dds://127.0.0.1:"),
            "{}",
            arrived.origin_uri
        );
        assert!(
            arrived.origin_uri.ends_with("00000102?sn=1"),
            "{}",
            arrived.origin_uri
        );
        let second = loopback.round(b"again").expect("round");
        assert!(
            second.origin_uri.ends_with("?sn=1"),
            "a fresh writer each round"
        );
        assert!(loopback.ceiling().is_none());
        assert!(loopback.refuses(b"anything").is_none());
        assert_eq!(loopback.name(), "dds");
        assert!(loopback.directions().receives() && loopback.directions().sends());
        assert!(loopback.claims().is_none());
    }

    #[test]
    fn the_loopback_returns_the_edges_whole() {
        let loopback = DdsTransport::loopback();
        for (name, bytes) in edge_payloads() {
            let arrived = loopback
                .round(&bytes)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(arrived.bytes, bytes, "{name}");
        }
    }

    #[test]
    fn a_writer_counts_its_samples_and_a_target_names_the_locator() {
        let reader = DdsTransport::loopback();
        let (socket, locator) = reader.bind().expect("binding");
        let writer = DdsTransport::loopback();
        let target = format!("dds://{locator}");
        let writing = std::thread::spawn(move || {
            writer
                .send(&target, b"one")
                .and_then(|()| writer.send(&target, b"two"))
        });
        let first = reader.read_one(&socket).expect("reading").expect("one");
        let second = reader.read_one(&socket).expect("reading").expect("two");
        writing.join().expect("thread").expect("writing");
        assert_eq!(first.bytes, b"one");
        assert_eq!(second.bytes, b"two");
        assert!(first.origin_uri.ends_with("?sn=1") && second.origin_uri.ends_with("?sn=2"));
        assert!(
            reader.receive().expect("nobody").is_empty(),
            "nobody is not an error"
        );
    }

    #[test]
    fn a_datagram_that_is_not_rtps_is_refused_by_the_reader() {
        let reader = DdsTransport::loopback();
        let (socket, locator) = reader.bind().expect("binding");
        UdpTransport::new("127.0.0.1:0")
            .send(&locator, b"not rtps at all")
            .expect("sending");
        assert!(reader.read_one(&socket).is_err());
    }
}
