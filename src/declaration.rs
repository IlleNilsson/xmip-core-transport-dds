//! What a Location of this technology is configured with: its declaration,
//! which `Configured` reads, built with the crate's `declaration` feature
//! alone and the code behind the default (ADR-0064, amendments 2026-10-22
//! and 2026-11-26), for the library the surfaces load to carry (open
//! problem 48). The read window and the socket's hold a Location takes
//! unless told otherwise are declared here, and the code takes them from
//! here.

use std::time::Duration;

use xcore::settings::{Applies, Fixed, Kind, Presence, Setting, Settings};

/// How long a read window lasts when a Location says nothing else.
pub const TIMEOUT: Duration = Duration::from_secs(5);

/// How many bytes of datagrams a reader's socket holds when a Location says
/// nothing else: a mebibyte sample's fragments four times over. Best-effort
/// RTPS repairs nothing, so a fragment the socket could not hold loses the
/// sample.
pub const BUFFER: u32 = 4 << 20;

/// Every setting a Location takes beyond its address, the local socket the
/// participant binds: the locator a Receive Location's reader is at, the
/// one a Send Location writes from.
pub const SETTINGS: &Settings = &Settings {
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
        transport::configured::timeout(
            Applies::Receive,
            Some(TIMEOUT),
            "How long a reader waits for a sample, or for its next fragment.",
        ),
        Setting {
            name: "buffer",
            kind: Kind::Integer {
                minimum: 65_536,
                maximum: 1 << 30,
            },
            presence: Presence::Default(Fixed::Integer(BUFFER as i64)),
            meaning: "How many bytes of datagrams a reader's socket holds: a sample's \
                      fragments arrive back to back, and one the socket cannot hold \
                      loses the sample.",
            applies: Applies::Receive,
        },
    ],
};

/// The schemes a Location's address is entered with to choose this technology.
pub const SCHEMES: &[&str] = &["dds"];
