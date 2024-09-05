use hickory_resolver::{error::ResolveError, TokioAsyncResolver};
use libp2p::Multiaddr;
use std::str::FromStr;

pub async fn resolve_peers_from_dns() -> Result<Vec<Multiaddr>, ResolveError> {
    let (config, mut opts) = hickory_resolver::system_conf::read_system_conf()?;

    opts.edns0 = true;
    opts.try_tcp_on_error = true;

    let resolver = TokioAsyncResolver::tokio(config, opts);
    let records = resolver.txt_lookup("_dnsaddr.splash.dexie.space.").await?;

    let peers: Vec<Multiaddr> = records
        .iter()
        .flat_map(|record| record.txt_data())
        .filter_map(|txt| std::str::from_utf8(txt).ok())
        .map(|addr_str| addr_str.trim_start_matches("dnsaddr="))
        .filter_map(|addr_str| Multiaddr::from_str(addr_str).ok())
        .collect();

    if peers.is_empty() {
        Err(ResolveError::from("No peers found"))
    } else {
        Ok(peers)
    }
}
