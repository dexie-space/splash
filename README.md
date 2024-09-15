# Splash!

Splash! is a decentralized network for sharing [Offers](https://chialisp.com/offers/) across the [Chia](https://github.com/Chia-Network/chia-blockchain) ecosystem based on Rusts [libp2p](https://github.com/libp2p/js-libp2p) with DHT peer discovery.

Every connected peer receives all offers broadcasted from other peers. There is no centralized connection; peers connect to each other and are aware of each other, enabling true peer-to-peer DeFi. It also provides privacy, as it is difficult to trace where an offer in the network originated.

The Splash! command line tool acts as a proxy between your application and the Splash! network. It will broadcast your offers to the network and relay offers from other peers to your local application through a local HTTP API.

## Installation

You can download prebuilt binaries in the
[releases section](https://github.com/dexie-space/splash/releases).

## Building

You can also build and install from source (requires the latest stable [Rust] compiler.)

```
cargo install splash
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
  -t, --testnet
          Use Testnet
      --offer-hook <OFFER_HOOK>
          HTTP endpoint where incoming offers are posted to, sends JSON body {"offer":"offer1..."} (defaults to STDOUT)
      --listen-offer-submission <HOST:PORT>
          Start a HTTP API for offer submission, expects JSON body {"offer":"offer1..."}
      --listen-metrics <HOST:PORT>
          Start a HTTP API for metrics
  -h, --help
          Print help
  -V, --version
          Print version
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

`./splash --identity-file identity.json`

## Using Splash with Docker

```bash
docker run -p 11511:11511 -p 4000:4000 dexiespace/splash:latest \
--listen-offer-submission 0.0.0.0:4000 \
--listen-address /ip4/0.0.0.0/tcp/11511
# send the request
curl -X POST -H "Content-Type: application/json" -d '{"offer":"offer1..."}' http://localhost:4000
```

## Becoming a stable peer

If you run a permanent node, it is recommended that you become a stable peer. This requires opening an inbound port in your firewall. Then, start your node with the `--listen-address` option, specifying your public interface and the selected port (e.g., `11511`).

`./splash --listen-address /ip6/2001:db8::1/tcp/11511 --listen-address /ip4/1.2.3.4/tcp/11511`

Note: If you run Splash behind a NAT, make sure to forward the port to your local IP and listen on that local IP. Splash will detect and announce your external IP accordingly.

## Hardware requirements

Splash is designed to be lightweight, does not require disk I/O, and should run on basically any hardware, including a 1st-gen Raspberry Pi. Network bandwidth usage is minimal but will increase with the number of broadcasted offers.

## Splash Indexing

To keep Splash as lightweight as possible, it does not index or store any offers it receives; they are simply forwarded to all connected peers or to the local HTTP hook. To find past offers or their status, you need to track them locally or use a service that indexes the offers.

One such service is [dexie.space](https://dexie.space). dexie observes the Splash network, indexes all offers for easy search and retrieval, and keeps the index up to date. You can use the [dexie API](https://dexie.space/api) to search for offers, get offer details, and more.

## Using Splash Programmatically

Splash can be integrated into any Rust project. Here's how to do it:

1. Add Splash and tokio to your `Cargo.toml`:

```toml
[dependencies]
splash = { git = "https://github.com/dexie-space/splash"}
tokio = "1.40.0"
```

2. In your Rust code, initialize Splash and listen for events:

```rust
use splash::{Splash, SplashEvent, SplashContext};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let SplashContext { node, mut events } = Splash::new().build().await?;

    // Submit an offer
    // node.broadcast_offer("offer1...").await?;

    // Process events
    while let Some(event) = events.recv().await {
        match event {
            SplashEvent::Initialized(peer_id) => println!("Our Peer ID: {}", peer_id),
            SplashEvent::NewListenAddress(address) => println!("Listening on: {}", address),
            SplashEvent::PeerConnected(peer_id) => println!("Connected to peer: {}", peer_id),
            SplashEvent::PeerDisconnected(peer_id) => println!("Disconnected from peer: {}", peer_id),
            SplashEvent::OfferReceived(offer) => println!("Received offer: {}", offer),
            SplashEvent::OfferBroadcasted(offer) => println!("Broadcasted offer: {}", offer),
            SplashEvent::OfferBroadcastFailed(err) => println!("Failed to broadcast offer: {}", err),
        }
    }

    Ok(())
}
```

## Building alternative clients

The Splash network is based on [libp2p](https://libp2p.io), meaning any libp2p library should be able to connect to the network. Use the following identifiers:

- Kademlia Protocol: `/splash/kad/1`
- Identify Protocol: `/splash/id/1`
- Gossipsub Subscription: `/splash/offers/1`

An optional list of initially reachable peers can be requested via DNS TXT from `_dnsaddr.splash.dexie.space`.
