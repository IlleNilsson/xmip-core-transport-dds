# xmip-core-transport-dds

DDS transport: RTPS over UDP — DATA submessages carry a sample whole and DATA_FRAG submessages carry it in fragments reassembled by sequence number; a Location is a participant's unicast locator; an in-process reader stands in for the domain. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

A Receive Location keeps its socket, bound on the first receive (`transport::kept::Kept`): a datagram that arrives between two receives waits in its buffer for the next, where until 2026-09-27 each receive bound a socket of its own and a datagram sent between receives was lost.

A reader's socket holds `buffer` bytes of datagrams, 4 MiB by default (`transport::socket::hold`): a sample's fragments arrive back to back and best-effort RTPS repairs none, so a fragment the socket cannot hold loses the sample. Until 2026-10-06 the socket held what the operating system gave it, 64 KiB on Windows, one fragment run; a mebibyte sample lost a fragment whenever its reader was not scheduled in time, and four more quiet read windows hid it as a slow writer. Linux caps the buffer at `net.core.rmem_max`.

The peer a datagram came from is read by `udp::peer_of`, where UDP writes the origin; until 2026-09-28 this technology cut it out of UDP's origin itself.

## Acknowledgement

Acceptance is at-most-once here. The writer is best-effort RTPS: it sends no
HEARTBEAT and waits for no ACKNACK, so nothing is said back to it and it is
never told how the receive cycle ended; a crash before the Stream is durable
loses the sample. Reliable RTPS, whose ACKNACK could carry the verdict, is not
here. Each sample arrives whole, its fragments put back together first.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
