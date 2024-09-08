use clap::Parser;
use libp2p::identity;
use libp2p::Multiaddr;
use serde_json::json;
use splash::{Splash, SplashContext, SplashEvent};
use std::net::SocketAddr;
use warp::http::StatusCode;
use warp::Filter;
mod metrics;
mod utils;

#[derive(Parser, Debug)]
#[clap(name = "Splash!", version = env!("CARGO_PKG_VERSION"))]
struct Opt {
    #[clap(
        long,
        short,
        value_name = "MULTIADDR",
        help = "Set initial peer, if missing use dexies DNS introducer"
    )]
    known_peer: Vec<Multiaddr>,

    #[clap(
        long,
        short,
        value_name = "MULTIADDR",
        help = "Set listen address, defaults to all interfaces, use multiple times for multiple addresses"
    )]
    listen_address: Vec<Multiaddr>,

    #[clap(
        long,
        short,
        help = "Store and reuse peer identity (only useful for known peers)"
    )]
    identity_file: Option<String>,

    #[clap(
        long,
        help = "HTTP endpoint where incoming offers are posted to, sends JSON body {\"offer\":\"offer1...\"} (defaults to STDOUT)"
    )]
    offer_hook: Option<String>,

    #[clap(
        long,
        help = "Start a HTTP API for offer submission, expects JSON body {\"offer\":\"offer1...\"}",
        value_name = "HOST:PORT"
    )]
    listen_offer_submission: Option<String>,

    #[clap(long, help = "Start a HTTP API for metrics", value_name = "HOST:PORT")]
    listen_metrics: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::parse();

    println!("Welcome to Splash! v{}", env!("CARGO_PKG_VERSION"));

    let mut splash = Splash::new()
        .with_listen_addresses(opt.listen_address)
        .with_known_peers(opt.known_peer);

    // Load or generate peer identity (keypair), only if --identity-file is specified
    if let Some(keypair) = opt.identity_file.as_ref().map(|file_path| {
        utils::load_keypair_from_file(file_path).unwrap_or_else(|_| {
            let keypair = identity::Keypair::generate_ed25519();
            utils::save_keypair_to_file(&keypair, file_path).ok();
            keypair
        })
    }) {
        splash = splash.with_keys(keypair);
    }

    let SplashContext { node, mut events } = splash.build().await?;

    let metrics = metrics::Metrics::new();

    // Start a local webserver for offer submission, only if --listen-offer-submission is specified
    if let Some(offer_submission_addr_str) = opt.listen_offer_submission {
        let offer_route =
            warp::post()
                .and(warp::body::json())
                .and_then(move |offer: serde_json::Value| {
                    let node = node.clone();
                    async move {
                        let response =
                            if let Some(offer_str) = offer.get("offer").and_then(|v| v.as_str()) {
                                match node.broadcast_offer(offer_str).await {
                                    Ok(_) => warp::reply::json(&json!({"success": true})),
                                    Err(e) => warp::reply::json(&json!({
                                        "success": false,
                                        "error": e.to_string(),
                                    })),
                                }
                            } else {
                                warp::reply::json(&json!({
                                    "success": false,
                                    "error": "Invalid offer format",
                                }))
                            };

                        Ok::<_, warp::Rejection>(warp::reply::with_status(response, StatusCode::OK))
                    }
                });

        let submission_addr: SocketAddr = offer_submission_addr_str.parse()?;

        tokio::spawn(async move {
            warp::serve(offer_route).run(submission_addr).await;
        });
    }

    // Start a local webserver for splash metrics, only if --listen-metrics is specified
    if let Some(listen_metrics_str) = opt.listen_metrics {
        let metrics_address: SocketAddr = listen_metrics_str.parse()?;

        let metrics = metrics.clone();
        let metrics_route = warp::get().map(move || {
            let metrics_data = metrics.get_metrics();
            warp::reply::json(&metrics_data)
        });

        tokio::spawn(async move {
            warp::serve(metrics_route).run(metrics_address).await;
        });
    }

    // Process the received events
    while let Some(event) = events.recv().await {
        match event {
            SplashEvent::Initialized(peer_id) => println!("Our Peer ID: {}", peer_id),

            SplashEvent::NewListenAddress(address) => println!("Listening on: {}", address),

            SplashEvent::PeerConnected(peer_id) => {
                let peers = metrics.increment_peers();
                println!("Connected to peer: {} (peers: {})", peer_id, peers);
            }

            SplashEvent::PeerDisconnected(peer_id) => {
                let peers = metrics.decrement_peers();
                println!("Disconnected from peer: {} (peers: {})", peer_id, peers);
            }

            SplashEvent::OfferBroadcasted(offer) => {
                println!("Broadcasted Offer: {}", offer);
                metrics.increment_offers_broadcasted();
            }

            SplashEvent::OfferBroadcastFailed(err) => {
                println!("Broadcasting Offer failed: {}", err)
            }

            SplashEvent::OfferReceived(offer) => {
                println!("Received Offer: {}", offer);
                metrics.increment_offers_received();

                if let Some(ref endpoint_url) = opt.offer_hook {
                    let endpoint_url_clone = endpoint_url.clone();
                    tokio::spawn(async move {
                        if let Err(e) = utils::offer_post_hook(&endpoint_url_clone, &offer).await {
                            eprintln!("Error posting to offer hook: {}", e);
                        }
                    });
                }
            }
        }
    }

    Ok(())
}
