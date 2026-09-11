//! The RTPS message as a datagram carries it: a header naming the protocol,
//! its version, the vendor and the sender's GUID prefix, then submessages —
//! each an identifier, flags and a length — of which this crate carries
//! `INFO_TS`, `DATA` with a sample whole, and `DATA_FRAG` with a run of
//! fragments of one. A sample is an octet sequence in CDR, little-endian,
//! behind the encapsulation header.

use transport::error::{Result, protocol_error};

/// Version 2.3 of the wire protocol.
pub const VERSION: [u8; 2] = [2, 3];
/// The vendor identifier nobody was assigned.
pub const VENDOR_UNKNOWN: [u8; 2] = [0, 0];
/// The reader every writer may address.
pub const ENTITYID_UNKNOWN: [u8; 4] = [0; 4];
/// A user-defined writer with a key.
pub const WRITER_WITH_KEY: [u8; 4] = [0x00, 0x00, 0x01, 0x02];
/// How many bytes one fragment holds.
pub const FRAGMENT_SIZE: u16 = 1024;
/// How many fragments one `DATA_FRAG` submessage carries: sixty-three of
/// them stay inside one datagram.
pub const FRAGMENTS_PER_SUBMESSAGE: u16 = 63;
/// The most serialized payload a `DATA` submessage carries in one datagram:
/// udp's datagram less the message header, the timestamp and the
/// submessage's own header.
pub const MAX_DATA_PAYLOAD: usize = udp::MAX_DATAGRAM - 20 - 12 - 24;

/// CDR, little-endian, no options.
const CDR_LE: [u8; 4] = [0x00, 0x01, 0x00, 0x00];
const INFO_TS: u8 = 0x09;
const DATA: u8 = 0x15;
const DATA_FRAG: u8 = 0x16;
/// The endianness flag: little.
const FLAG_ENDIAN: u8 = 0x01;
/// `DATA` carries serialized data.
const FLAG_DATA: u8 = 0x04;

/// One submessage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submessage {
    InfoTimestamp {
        seconds: i32,
        fraction: u32,
    },
    Data {
        writer_id: [u8; 4],
        sequence: i64,
        payload: Vec<u8>,
    },
    DataFrag {
        writer_id: [u8; 4],
        sequence: i64,
        /// The first fragment carried, counted from one.
        starting: u32,
        count: u16,
        size: u16,
        sample_size: u32,
        payload: Vec<u8>,
    },
}

/// One message: who sent it, and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub guid_prefix: [u8; 12],
    pub submessages: Vec<Submessage>,
}

fn sequence_bytes(sequence: i64) -> [u8; 8] {
    let high = i32::try_from(sequence >> 32).unwrap_or(i32::MAX);
    let low = u32::try_from(sequence & 0xffff_ffff).unwrap_or(u32::MAX);
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&high.to_le_bytes());
    out[4..].copy_from_slice(&low.to_le_bytes());
    out
}

fn sequence_of(bytes: &[u8]) -> i64 {
    let high = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let low = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    (i64::from(high) << 32) | i64::from(low)
}

impl Submessage {
    fn encode(&self, out: &mut Vec<u8>) {
        let (id, flags, body): (u8, u8, Vec<u8>) = match self {
            Self::InfoTimestamp { seconds, fraction } => {
                let mut body = seconds.to_le_bytes().to_vec();
                body.extend_from_slice(&fraction.to_le_bytes());
                (INFO_TS, FLAG_ENDIAN, body)
            }
            Self::Data {
                writer_id,
                sequence,
                payload,
            } => {
                let mut body = vec![0, 0, 16, 0];
                body.extend_from_slice(&ENTITYID_UNKNOWN);
                body.extend_from_slice(writer_id);
                body.extend_from_slice(&sequence_bytes(*sequence));
                body.extend_from_slice(payload);
                (DATA, FLAG_ENDIAN | FLAG_DATA, body)
            }
            Self::DataFrag {
                writer_id,
                sequence,
                starting,
                count,
                size,
                sample_size,
                payload,
            } => {
                let mut body = vec![0, 0, 28, 0];
                body.extend_from_slice(&ENTITYID_UNKNOWN);
                body.extend_from_slice(writer_id);
                body.extend_from_slice(&sequence_bytes(*sequence));
                body.extend_from_slice(&starting.to_le_bytes());
                body.extend_from_slice(&count.to_le_bytes());
                body.extend_from_slice(&size.to_le_bytes());
                body.extend_from_slice(&sample_size.to_le_bytes());
                body.extend_from_slice(payload);
                (DATA_FRAG, FLAG_ENDIAN, body)
            }
        };
        out.push(id);
        out.push(flags);
        out.extend_from_slice(&u16::try_from(body.len()).unwrap_or(u16::MAX).to_le_bytes());
        out.extend(body);
    }

    fn decode(id: u8, flags: u8, body: &[u8]) -> Result<Option<Self>> {
        if flags & FLAG_ENDIAN == 0 {
            return Err(protocol_error(
                "a big-endian submessage this crate does not read",
            ));
        }
        let cut = || protocol_error("a submessage cut off inside its header");
        match id {
            INFO_TS => {
                let bytes = body.get(..8).ok_or_else(cut)?;
                Ok(Some(Self::InfoTimestamp {
                    seconds: i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                    fraction: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
                }))
            }
            DATA => {
                let head = body.get(..20).ok_or_else(cut)?;
                if flags & FLAG_DATA == 0 {
                    return Err(protocol_error("a DATA without data"));
                }
                Ok(Some(Self::Data {
                    writer_id: [head[8], head[9], head[10], head[11]],
                    sequence: sequence_of(&head[12..20]),
                    payload: body[20..].to_vec(),
                }))
            }
            DATA_FRAG => {
                let head = body.get(..32).ok_or_else(cut)?;
                Ok(Some(Self::DataFrag {
                    writer_id: [head[8], head[9], head[10], head[11]],
                    sequence: sequence_of(&head[12..20]),
                    starting: u32::from_le_bytes([head[20], head[21], head[22], head[23]]),
                    count: u16::from_le_bytes([head[24], head[25]]),
                    size: u16::from_le_bytes([head[26], head[27]]),
                    sample_size: u32::from_le_bytes([head[28], head[29], head[30], head[31]]),
                    payload: body[32..].to_vec(),
                }))
            }
            // A submessage this crate does not read is skipped, as the
            // specification says a reader must.
            _ => Ok(None),
        }
    }
}

impl Message {
    /// The message as the datagram carries it.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = b"RTPS".to_vec();
        out.extend_from_slice(&VERSION);
        out.extend_from_slice(&VENDOR_UNKNOWN);
        out.extend_from_slice(&self.guid_prefix);
        for submessage in &self.submessages {
            submessage.encode(&mut out);
        }
        out
    }

    /// The message `bytes` carry, submessages this crate does not read
    /// skipped.
    ///
    /// # Errors
    /// Not an RTPS 2 header, or a submessage cut off.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let (head, mut rest) = bytes
            .split_at_checked(20)
            .ok_or_else(|| protocol_error("a message cut off inside its header"))?;
        if &head[..4] != b"RTPS" || head[4] != 2 {
            return Err(protocol_error("not an RTPS 2 message"));
        }
        let mut guid_prefix = [0u8; 12];
        guid_prefix.copy_from_slice(&head[8..20]);
        let mut submessages = Vec::new();
        while !rest.is_empty() {
            let (header, after) = rest
                .split_at_checked(4)
                .ok_or_else(|| protocol_error("a submessage cut off inside its header"))?;
            let length = usize::from(u16::from_le_bytes([header[2], header[3]]));
            // A length of zero means the submessage runs to the end.
            let length = if length == 0 { after.len() } else { length };
            let (body, after) = after
                .split_at_checked(length)
                .ok_or_else(|| protocol_error("a submessage cut off before its length"))?;
            if let Some(submessage) = Submessage::decode(header[0], header[1], body)? {
                submessages.push(submessage);
            }
            rest = after;
        }
        Ok(Self {
            guid_prefix,
            submessages,
        })
    }
}

/// `bytes` as a CDR octet sequence behind the encapsulation header, padded
/// to four.
#[must_use]
pub fn serialize(bytes: &[u8]) -> Vec<u8> {
    let mut out = CDR_LE.to_vec();
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(bytes);
    out.resize(out.len().next_multiple_of(4), 0);
    out
}

/// The octet sequence a serialized payload carries.
///
/// # Errors
/// Not CDR little-endian, or a length past the payload.
pub fn deserialize(serialized: &[u8]) -> Result<Vec<u8>> {
    let (head, rest) = serialized
        .split_at_checked(8)
        .ok_or_else(|| protocol_error("a payload cut off inside its encapsulation"))?;
    if head[..4] != CDR_LE {
        return Err(protocol_error("an encapsulation this crate does not read"));
    }
    let length = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
    rest.get(..usize::try_from(length).unwrap_or(usize::MAX))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| protocol_error("a length past the payload"))
}

/// The messages that carry `serialized` as sample `sequence` from
/// `writer_id`: one `DATA` where it fits a datagram, else `DATA_FRAG`s.
#[must_use]
pub fn messages(
    guid_prefix: [u8; 12],
    writer_id: [u8; 4],
    sequence: i64,
    serialized: &[u8],
) -> Vec<Message> {
    let stamp = Submessage::InfoTimestamp {
        seconds: 0,
        fraction: 0,
    };
    if serialized.len() <= MAX_DATA_PAYLOAD {
        return vec![Message {
            guid_prefix,
            submessages: vec![
                stamp,
                Submessage::Data {
                    writer_id,
                    sequence,
                    payload: serialized.to_vec(),
                },
            ],
        }];
    }
    let per_message = usize::from(FRAGMENT_SIZE) * usize::from(FRAGMENTS_PER_SUBMESSAGE);
    serialized
        .chunks(per_message)
        .enumerate()
        .map(|(index, run)| Message {
            guid_prefix,
            submessages: vec![
                stamp.clone(),
                Submessage::DataFrag {
                    writer_id,
                    sequence,
                    starting: u32::try_from(index * usize::from(FRAGMENTS_PER_SUBMESSAGE) + 1)
                        .unwrap_or(u32::MAX),
                    count: u16::try_from(run.len().div_ceil(usize::from(FRAGMENT_SIZE)))
                        .unwrap_or(u16::MAX),
                    size: FRAGMENT_SIZE,
                    sample_size: u32::try_from(serialized.len()).unwrap_or(u32::MAX),
                    payload: run.to_vec(),
                },
            ],
        })
        .collect()
}

/// One sample being put back together from its fragments.
#[derive(Debug, Default)]
pub struct Reassembly {
    sequence: Option<i64>,
    bytes: Vec<u8>,
    filled: usize,
}

impl Reassembly {
    /// One submessage; the serialized sample when this completes it. A
    /// timestamp completes nothing.
    ///
    /// # Errors
    /// A fragment past the sample's size.
    pub fn take(&mut self, submessage: &Submessage) -> Result<Option<Vec<u8>>> {
        match submessage {
            Submessage::InfoTimestamp { .. } => Ok(None),
            Submessage::Data { payload, .. } => Ok(Some(payload.clone())),
            Submessage::DataFrag {
                sequence,
                starting,
                size,
                sample_size,
                payload,
                ..
            } => {
                if self.sequence != Some(*sequence) {
                    *self = Self {
                        sequence: Some(*sequence),
                        bytes: vec![0; usize::try_from(*sample_size).unwrap_or(0)],
                        filled: 0,
                    };
                }
                let at =
                    usize::try_from(starting.saturating_sub(1)).unwrap_or(0) * usize::from(*size);
                let slot = self
                    .bytes
                    .get_mut(at..at + payload.len())
                    .ok_or_else(|| protocol_error("a fragment past the sample's size"))?;
                slot.copy_from_slice(payload);
                self.filled += payload.len();
                if self.filled >= self.bytes.len() {
                    self.sequence = None;
                    return Ok(Some(std::mem::take(&mut self.bytes)));
                }
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

    #[test]
    fn a_small_sample_is_one_data_submessage_that_reads_back() {
        let serialized = serialize(b"hello");
        assert_eq!(&serialized[..8], &[0, 1, 0, 0, 5, 0, 0, 0]);
        assert_eq!(serialized.len(), 16, "padded to four");
        assert_eq!(deserialize(&serialized).expect("deserialize"), b"hello");
        let sent = messages(PREFIX, WRITER_WITH_KEY, 7, &serialized);
        assert_eq!(sent.len(), 1);
        let bytes = sent[0].encode();
        assert_eq!(&bytes[..8], b"RTPS\x02\x03\x00\x00");
        assert_eq!(&bytes[20..24], &[INFO_TS, 0x01, 8, 0]);
        assert_eq!(&bytes[32..36], &[DATA, 0x05, 36, 0]);
        let read = Message::decode(&bytes).expect("decode");
        assert_eq!(read, sent[0]);
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.take(&read.submessages[0]).expect("stamp"), None);
        let whole = reassembly.take(&read.submessages[1]).expect("data");
        assert_eq!(whole, Some(serialized));
    }

    #[test]
    fn a_large_sample_is_fragments_that_come_back_together() {
        let sample: Vec<u8> = (0..200_000u32)
            .map(|n| u8::try_from(n % 251).unwrap_or(0))
            .collect();
        let serialized = serialize(&sample);
        let sent = messages(PREFIX, WRITER_WITH_KEY, 8, &serialized);
        assert_eq!(sent.len(), 4, "200 008 bytes in runs of 64 512");
        assert!(sent.iter().all(|m| m.encode().len() <= udp::MAX_DATAGRAM));
        let mut reassembly = Reassembly::default();
        let mut whole = None;
        for message in &sent {
            let read = Message::decode(&message.encode()).expect("decode");
            assert_eq!(read, *message);
            for submessage in &read.submessages {
                assert!(whole.is_none(), "not before the last");
                whole = reassembly.take(submessage).expect("fragment");
            }
        }
        assert_eq!(whole.as_deref(), Some(serialized.as_slice()));
        assert_eq!(deserialize(&serialized).expect("deserialize"), sample);
        let Submessage::DataFrag {
            starting, count, ..
        } = &sent[1].submessages[1]
        else {
            panic!("a fragment run");
        };
        assert_eq!((*starting, *count), (64, 63));
    }

    #[test]
    fn what_is_not_a_message_is_refused_and_the_unknown_is_skipped() {
        let bytes = messages(PREFIX, WRITER_WITH_KEY, 1, &serialize(b"x"))[0].encode();
        assert!(Message::decode(&bytes[..19]).is_err(), "cut off");
        assert!(
            Message::decode(&bytes[..30]).is_err(),
            "inside a submessage"
        );
        let mut bad = bytes.clone();
        bad[4] = 1;
        assert!(Message::decode(&bad).is_err(), "RTPS 1");
        let mut bad = bytes.clone();
        bad[21] = 0x00;
        assert!(Message::decode(&bad).is_err(), "big-endian");
        let mut heartbeat = bytes[..20].to_vec();
        heartbeat.extend_from_slice(&[0x07, 0x01, 4, 0, 0, 0, 0, 0]);
        let read = Message::decode(&heartbeat).expect("skipped");
        assert!(read.submessages.is_empty());
        assert!(
            deserialize(&[0, 1, 0, 0, 9, 0, 0, 0, 1]).is_err(),
            "length past"
        );
        assert!(
            deserialize(&[0, 0, 0, 0, 0, 0, 0, 0]).is_err(),
            "big-endian CDR"
        );
        let stray = Submessage::DataFrag {
            writer_id: WRITER_WITH_KEY,
            sequence: 2,
            starting: 9,
            count: 1,
            size: 1024,
            sample_size: 100,
            payload: vec![0; 10],
        };
        assert!(Reassembly::default().take(&stray).is_err(), "past the size");
    }
}
