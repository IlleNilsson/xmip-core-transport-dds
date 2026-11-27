# xmip-core-transport-dds

DDS transport: RTPS over UDP — DATA submessages carry a sample whole and DATA_FRAG submessages carry it in fragments reassembled by sequence number; a Location is a participant's unicast locator; an in-process reader stands in for the domain. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

A Receive Location keeps its socket, bound as its transport is opened (`Configured::listening`; on its first receive until 2026-11-16) (`transport::kept::Kept`): a datagram that arrives between two receives waits in its buffer for the next, where until 2026-09-27 each receive bound a socket of its own and a datagram sent between receives was lost.

A reader's socket holds `buffer` bytes of datagrams, 4 MiB by default (`transport::socket::hold`): a sample's fragments arrive back to back and best-effort RTPS repairs none, so a fragment the socket cannot hold loses the sample. Until 2026-10-06 the socket held what the operating system gave it, 64 KiB on Windows, one fragment run; a mebibyte sample lost a fragment whenever its reader was not scheduled in time, and four more quiet read windows hid it as a slow writer. Linux caps the buffer at `net.core.rmem_max`.

The peer a datagram came from is read by `udp::peer_of`, where UDP writes the origin; until 2026-09-28 this technology cut it out of UDP's origin itself.

## Acknowledgement

Acceptance is at-most-once here. The writer is best-effort RTPS: it sends no
HEARTBEAT and waits for no ACKNACK, so nothing is said back to it and it is
never told how the receive cycle ended; a crash before the Stream is durable
loses the sample. Reliable RTPS, whose ACKNACK could carry the verdict, is not
here. Each sample arrives whole, its fragments put back together first.

## The declaration

What a Location of this technology is configured with is two `const`s in its `declaration` module, `src/declaration.rs`, beside the code, which `Configured` reads (ADR-0064, amendment 2026-11-26): `declaration::SETTINGS`, `locator`, `timeout` and `buffer`, and `SCHEMES`, `dds`. The read window and the socket's hold a Location takes unless told otherwise, `declaration::TIMEOUT` and `BUFFER`, are declared there too, and the code takes them from there; until 2026-11-26 they were the crate root's, and the meaning of `buffer` carried runs of spaces where its line breaks had been. Built with its `declaration` feature alone (`--no-default-features --features declaration`), the crate is those and nothing more: its code and every crate the code is built on — the udp technology among them — are behind the default feature. So the library the surfaces load carries it without the code, through the gathering crate xmip-core-declaration (open problem 48), and the language server and the desktop suggest and check a DDS Location's settings though no node in their process loaded the technology. `test/Declaration.Test.ps1` in the estate holds that the declaration builds nothing the transport capability does not.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
