use hickory_resolver::{
    net::NetError,
    proto::rr::{RData, Record},
    TokioResolver,
};
use libp2p::Multiaddr;
use std::str::FromStr;

pub async fn resolve_peers_from_dns(network_name: String) -> Result<Vec<Multiaddr>, NetError> {
    let mut builder = TokioResolver::builder_tokio()?;
    builder.options_mut().edns0 = true;
    builder.options_mut().try_tcp_on_error = true;

    let resolver = builder.build()?;
    let records = resolver
        .txt_lookup(format!("_dnsaddr.{}.dexie.space.", network_name))
        .await?;

    let peers = peers_from_records(records.answers());
    if peers.is_empty() {
        Err(NetError::from("No peers found"))
    } else {
        Ok(peers)
    }
}

fn peers_from_records(records: &[Record]) -> Vec<Multiaddr> {
    records
        .iter()
        .filter_map(|record| match &record.data {
            RData::TXT(txt) => Some(txt),
            _ => None,
        })
        .flat_map(|record| record.txt_data.iter())
        .filter_map(|txt| std::str::from_utf8(txt).ok())
        .map(|addr_str| addr_str.trim_start_matches("dnsaddr="))
        .filter_map(|addr_str| Multiaddr::from_str(addr_str).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_resolver::proto::rr::{rdata::TXT, Name};

    #[test]
    fn extracts_peer_addresses_from_txt_answers() {
        let peer_id = libp2p::identity::Keypair::generate_ed25519()
            .public()
            .to_peer_id();
        let address = format!("/ip4/127.0.0.1/tcp/11511/p2p/{peer_id}");
        let records = [Record::from_rdata(
            Name::from_str("_dnsaddr.splash.dexie.space.").unwrap(),
            300,
            RData::TXT(TXT::new(vec![format!("dnsaddr={address}")])),
        )];
        assert_eq!(peers_from_records(&records), vec![address.parse().unwrap()]);
    }

    #[test]
    fn ignores_non_txt_and_malformed_answers() {
        let name = Name::from_str("_dnsaddr.splash.dexie.space.").unwrap();
        let records = [
            Record::from_rdata(name.clone(), 300, RData::A("127.0.0.1".parse().unwrap())),
            Record::from_rdata(
                name,
                300,
                RData::TXT(TXT::new(vec!["not a multiaddr".to_string()])),
            ),
        ];
        assert!(peers_from_records(&records).is_empty());
        assert!(peers_from_records(&[]).is_empty());
    }
}
