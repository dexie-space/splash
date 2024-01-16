# Splash!

Splash! is a decentralized network for sharing [Offers](https://chialisp.com/offers/) across the [Chia](https://github.com/Chia-Network/chia-blockchain) ecosystem based on Rusts [libp2p](https://github.com/libp2p/js-libp2p) with DHT peer discovery.

Every connected peer receives all offers broadcasted from other peers. There is no centralized connection, the peers connect to each other and are aware of each other.

## Installation

You can download prebuilt binaries in the
[releases section](https://github.com/dexie-space/splash/releases).

## Building

You can also build and install from source (requires the latest stable [Rust] compiler.)

```console
cargo install --git https://github.com/dexie-space/splash.git splash
```

## Usage

```
Usage: splash [OPTIONS]

Options:
  -k, --known-peer <MULTIADDR>         Set initial peer, if missing use dexies DNS introducer
  -l, --listen-address <MULTIADDR>     Set listen address, defaults to all interfaces, use multiple times for multiple addresses
  -i, --identity-file <IDENTITY_FILE>  Store and reuse peer identity (useful for operating a known peer)
  -h, --help                           Print help
```

## Examples

Start the node and listen on all interfaces (will use dexies DNS introducer):

`./splash`

Start a node and bootsrap from a known peer (will not use dexies DNS introducer):

`./splash --known-peer /ip6/::1/tcp/12345/p2p/12D3K...`

Start a node and listen on a specific interface/port:

`./splash --listen-address /ip6/::1/tcp/12345`

Start a node and reuse identity:

`./splash --identify-file identity.json`
