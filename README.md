# xmip-core-transport-dds

DDS transport: RTPS over UDP — DATA submessages carry a sample whole and DATA_FRAG submessages carry it in fragments reassembled by sequence number; a Location is a participant's unicast locator; an in-process reader stands in for the domain. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

A Receive Location keeps its socket, bound on the first receive (`transport::kept::Kept`): a datagram that arrives between two receives waits in its buffer for the next, where until 2026-09-27 each receive bound a socket of its own and a datagram sent between receives was lost.

The peer a datagram came from is read by `udp::peer_of`, where UDP writes the origin; until 2026-09-28 this technology cut it out of UDP's origin itself.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
