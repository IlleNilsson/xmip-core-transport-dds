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

use codec::hex;
use net::Target;
pub use rtps::{Message, Reassembly, Submessage};
use transport::bound::{Bound, Reading};
use transport::error::{Result, protocol_error};
use transport::kept::Kept;
use transport::loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback};
use transport::{Arrived, Configured, Directions, Transport};
use udp::UdpTransport;
use xcore::settings::{Applies, Fixed, Kind, Presence, Read, Setting, Settings};

/// How many quiet read windows a half-read sample is given before the reader
/// says the fragments stopped. Each window is the socket's own timeout, so
/// this is a patience, not a deadline in seconds.
const QUIET_WINDOWS: u8 = 4;

/// How long a read window lasts when a Location says nothing else.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// A writer at one locator, or a reader on one.
#[derive(Clone)]
pub struct DdsTransport {
    bind: String,
    locator: String,
    guid_prefix: [u8; 12],
    sequence: Arc<Mutex<i64>>,
    timeout: Duration,
    /// The reader's socket the first receive binds, and every receive reads.
    receiving: Kept<UdpSocket>,
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
            timeout: TIMEOUT,
            receiving: Kept::new(),
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
        let mut quiet = 0u8;
        loop {
            let datagram = match datagrams.receive_one(socket) {
                Ok(datagram) => datagram,
                Err(error) if error.retryable && first => return Ok(None),
                // A quiet moment part way through a sample is not the end of
                // it. A large sample is many fragments, and on an operating
                // system whose receive buffer is smaller than the burst — as
                // Linux's is, where Windows swallowed it whole and hid this
                // for months — the reader drains faster than the writer
                // refills and meets a timeout with fragments still to come.
                // Give the writer a few more windows before saying the
                // fragments stopped (found on Linux, 2026-09-19).
                Err(error) if error.retryable && quiet < QUIET_WINDOWS => {
                    quiet += 1;
                    continue;
                }
                Err(error) => return Err(error.at("the fragments stopped coming")),
            };
            first = false;
            quiet = 0;
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
                    let peer = udp::peer_of(&datagram.origin_uri);
                    let origin = format!(
                        "dds://{peer}/{}{}?sn={sequence}",
                        hex::encode(&message.guid_prefix),
                        hex::encode(writer_id)
                    );
                    return Ok(Some(Arrived::new(origin, rtps::deserialize(&serialized)?)));
                }
            }
        }
    }
}

impl Transport for DdsTransport {
    fn name(&self) -> &'static str {
        "dds"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    /// No writer writing is not an error: an empty vector. Read from the
    /// socket the first receive bound and kept, so a sample written between
    /// two receives waits in its buffer.
    fn receive(&self) -> Result<Vec<Arrived>> {
        let socket = self.receiving.bound(|| self.bind())?;
        Ok(self.read_one(socket)?.into_iter().collect())
    }

    /// `target` may name the reader's locator, `dds://host:7411`, overriding
    /// the transport's.
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        match Target::under(&["dds"], target).map(|named| (named.authority(), named.path())) {
            Some((locator, _)) if !locator.is_empty() => self.write(locator, bytes),
            _ => self.write(&self.locator, bytes),
        }
    }
}

impl Configured for DdsTransport {
    /// The address is the local socket the participant binds: the locator a
    /// Receive Location's reader is at, the one a Send Location writes from.
    const SETTINGS: &'static Settings = &Settings {
        technology: env!("CARGO_PKG_NAME"),
        settings: &[
            Setting {
                name: "locator",
                kind: Kind::Address,
                presence: Presence::Required,
                meaning: "The reader's locator a sample is written to where the target \
                          names none, as host and port.",
                applies: Applies::Send,
            },
            Setting {
                name: "timeout",
                kind: Kind::Duration,
                presence: Presence::Default(Fixed::Duration(TIMEOUT)),
                meaning: "How long a reader waits for a sample, or for its next fragment.",
                applies: Applies::Receive,
            },
        ],
    };

    fn configured(address: &str, settings: &Read) -> Result<Self> {
        // A reader writes nowhere: it has no locator to write to.
        let transport = Self::new(address, settings.optional_text("locator").unwrap_or(""));
        Ok(match settings.optional_duration("timeout") {
            Some(timeout) => transport.timing_out_after(timeout),
            None => transport,
        })
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

impl Reading for DdsTransport {
    /// A reader bound at its locator, waiting for its one sample.
    fn take_one(self, socket: &UdpSocket) -> Result<Arrived> {
        self.read_one(socket)?
            .ok_or_else(|| protocol_error("no sample arrived"))
    }
}

impl Loopback for DdsTransport {
    fn far_end(&self) -> Result<Box<dyn FarEnd>> {
        Ok(Box::new(Bound::new(self.clone(), self.bind()?)))
    }

    fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
        Self::new("127.0.0.1:0", address)
            .timing_out_after(self.timeout)
            .send("", payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use transport::payload::{edge_payloads, patterned};
    use xcore::settings::Given;

    #[test]
    fn every_receive_reads_the_socket_the_first_bound() {
        let receiver = DdsTransport::loopback();
        receiver.receiving.bound(|| receiver.bind()).expect("bound");
        let address = receiver.receiving.address().expect("address");
        let writer = DdsTransport::loopback();
        transport::kept::held_across_receives(&receiver, address, 5, move |at, payload| {
            writer.write(at, payload)
        });
    }

    #[test]
    fn dds_declares_its_settings_and_reads_through_them() {
        assert_eq!(DdsTransport::SETTINGS.problems(), Vec::<String>::new());
        let given = [(
            "locator".to_string(),
            Given::Text("10.0.0.7:7411".to_string()),
        )];
        let writer = DdsTransport::open("0.0.0.0:0", Applies::Send, &given).expect("writer");
        assert_eq!(writer.locator, "10.0.0.7:7411");
        assert_eq!(writer.timeout, TIMEOUT);
        let given = [("timeout".to_string(), Given::Text("1s".to_string()))];
        let reader = DdsTransport::open("0.0.0.0:7411", Applies::Receive, &given).expect("reader");
        assert_eq!(reader.timeout, Duration::from_secs(1));
        let Err(refused) = DdsTransport::open("0.0.0.0:0", Applies::Send, &[]) else {
            panic!("locator is required on a Send Location");
        };
        assert!(
            refused.message.contains("\"locator\""),
            "{}",
            refused.message
        );
    }

    /// The shapes a protocol breaks on, as the Playground lists them.
    fn payloads() -> Vec<(&'static str, Vec<u8>)> {
        let mut payloads = edge_payloads();
        payloads.extend([
            ("udp maximum", patterned(65_507)),
            ("sixteen bits plus one", patterned(65_537)),
            ("a mebibyte", patterned(1 << 20)),
        ]);
        payloads
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
        for (name, bytes) in payloads() {
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
