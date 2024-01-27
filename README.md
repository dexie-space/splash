# Splash!

Splash! is a decentralized network for sharing [Offers](https://chialisp.com/offers/) across the [Chia](https://github.com/Chia-Network/chia-blockchain) ecosystem based on Rusts [libp2p](https://github.com/libp2p/js-libp2p) with DHT peer discovery.

Every connected peer receives all offers broadcasted from other peers. There is no centralized connection, the peers connect to each other and are aware of each other.

The Splash! command line tools acts as a proxy between your application the Splash! network. It will broadcast your offers to the network and relay offers from other peers to your local application through a local HTTP API.

## Installation

You can download prebuilt binaries in the
[releases section](https://github.com/dexie-space/splash/releases).

## Building

You can also build and install from source (requires the latest stable [Rust] compiler.)

```
cargo install --git https://github.com/dexie-space/splash.git splash
```

## Usage

```
Usage: splash [OPTIONS]

Options:
  -k, --known-peer <MULTIADDR>
          Set initial peer, if missing use dexies DNS introducer
  -l, --listen-address <MULTIADDR>
          Set listen address, defaults to all interfaces, use multiple times for multiple addresses
  -i, --identity-file <IDENTITY_FILE>
          Store and reuse peer identity (only useful for known peers)
      --offer-hook <OFFER_HOOK>
          HTTP endpoint where incoming offers are posted to, sends JSON body {"offer":"offer1..."} (defaults to STDOUT)
      --listen-offer-submission <HOST:PORT>
          Start a HTTP API for offer submission, expects JSON body {"offer":"offer1..."}
  -h, --help
          Print help
```

## Examples

Start the node and listen on all interfaces (will use dexies DNS introducer):

`./splash`

Start a node and open local webserver for offer submission on port 4000:

`./splash --listen-offer-submission 127.0.0.1:4000`

Start a node and post incoming offers to a HTTP hook:

`./splash --offer-hook http://yourApi/v1/offers`

Start a node and bootstrap from a known peer (will not use dexies DNS introducer):

`./splash --known-peer /ip6/::1/tcp/12345/p2p/12D3K...`

Start a node and listen on a specific interface/port:

`./splash --listen-address /ip6/::1/tcp/12345`

Start a node and reuse identity:

`./splash --identify-file identity.json`

## Test the API with Docker

```bash
docker run -p 11511:11511 -p 4000:4000 dexie-space/splash:latest \
--listen-offer-submission 0.0.0.0:4000 \
--listen-address /ip4/0.0.0.0/tcp/11511
# send the request
curl -X POST -H "Content-Type: application/json" -d '{"offer":"offer1..."}' http://localhost:4000
```

## Become a stable peer

To become a stable peer, you need to open an inbound port in your firewall. Then start Splash! with the `--listen-address` option and choose your public interface and the selected port (eg. `11511`).

`./splash --listen-address /ip6/2001:db8::1/tcp/11511 --listen-address /ip4/1.2.3.4/tcp/11511`

Running a stable peer? Let us know! We will add you to the default bootstrap list.
