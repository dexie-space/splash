use clap::Parser;
use libp2p::identity;
use libp2p::Multiaddr;
use serde_json::json;
use splash::Splash;
use splash::SplashEvent;
use std::net::SocketAddr;
use warp::http::StatusCode;
use warp::Filter;
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::parse();

    // Load or generate peer identity (keypair), only if --identity-file is specified
    let keys = opt.identity_file.as_ref().map(|file_path| {
        utils::load_keypair_from_file(file_path).unwrap_or_else(|_| {
            let keypair = identity::Keypair::generate_ed25519();
            utils::save_keypair_to_file(&keypair, file_path).ok();
            keypair
        })
    });

    let (mut events, submission, peer_id) =
        Splash::new(opt.known_peer, opt.listen_address, keys).await?;

    println!("Our Peer ID: {}", peer_id);

    // Start a local webserver for offer submission, only if --listen-offer-submission is specified
    if let Some(offer_submission_addr_str) = opt.listen_offer_submission {
        let offer_route =
            warp::post()
                .and(warp::body::json())
                .and_then(move |offer: serde_json::Value| {
                    let submission = submission.clone();
                    async move {
                        let response = match offer.get("offer").and_then(|v| v.as_str()) {
                            Some(offer_str) => {
                                let offer_str = offer_str.to_owned(); // Clone the offer string to own it
                                match submission.submit_offer(&offer_str).await {
                                    Ok(_) => warp::reply::with_status(
                                        warp::reply::json(&json!({
                                            "success": true,
                                        })),
                                        StatusCode::OK,
                                    ),
                                    Err(e) => warp::reply::with_status(
                                        warp::reply::json(&json!({
                                            "success": false,
                                            "error": e.to_string(),
                                        })),
                                        StatusCode::BAD_REQUEST,
                                    ),
                                }
                            }
                            _ => warp::reply::with_status(
                                warp::reply::json(&json!({
                                    "success": false,
                                    "error": "Invalid offer format"
                                })),
                                StatusCode::BAD_REQUEST,
                            ),
                        };

                        Ok::<_, warp::Rejection>(response)
                    }
                });

        let submission_addr: SocketAddr = offer_submission_addr_str.parse()?;

        tokio::spawn(async move {
            warp::serve(offer_route).run(submission_addr).await;
        });
    }

    // Process the received events
    while let Some(event) = events.recv().await {
        match event {
            SplashEvent::NewListenAddress(address) => println!("Listening on: {}", address),

            SplashEvent::PeerConnected(peer_id) => println!("Connected to peer: {}", peer_id),

            SplashEvent::PeerDisconnected(peer_id) => {
                println!("Disconnected from peer: {}", peer_id)
            }

            SplashEvent::OfferBroadcasted(offer) => {
                println!("Broadcasted Offer: {}", offer)
            }

            SplashEvent::OfferBroadcastFailed(err) => {
                println!("Broadcasting Offer failed: {}", err)
            }

            SplashEvent::OfferReceived(offer) => {
                println!("Received Offer: {}", offer);

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
