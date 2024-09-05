use clap::Parser;
use libp2p::identity;
use libp2p::Multiaddr;
use splash;

mod utils;

#[derive(Parser, Debug)]
#[clap(name = "Splash!")]
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

    // Load or generate peer identity (keypair), only if identity_file is specified
    let keys = opt.identity_file.as_ref().map(|file_path| {
        utils::load_keypair_from_file(file_path).unwrap_or_else(|_| {
            let keypair = identity::Keypair::generate_ed25519();
            utils::save_keypair_to_file(&keypair, file_path).ok();
            keypair
        })
    });

    splash::run_splash(
        opt.known_peer,
        opt.listen_address,
        opt.offer_hook,
        opt.listen_offer_submission,
        keys,
    )
    .await
}
