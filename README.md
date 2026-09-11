# xmip-core-transport-dds

DDS transport: RTPS over UDP — DATA submessages carry a sample whole and DATA_FRAG submessages carry it in fragments reassembled by sequence number; a Location is a participant's unicast locator; an in-process reader stands in for the domain. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
