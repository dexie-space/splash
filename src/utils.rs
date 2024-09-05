use libp2p::identity;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io;

pub fn load_keypair_from_file(file_path: &str) -> io::Result<identity::Keypair> {
    let contents = fs::read_to_string(file_path)?;
    let keypair_json: IdentityJson = serde_json::from_str(&contents)?;
    identity::Keypair::from_protobuf_encoding(&keypair_json.identity)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Failed to decode keypair"))
}

pub fn save_keypair_to_file(keypair: &identity::Keypair, file_path: &str) -> io::Result<()> {
    let encoded = keypair
        .to_protobuf_encoding()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let file = File::create(file_path)?;
    serde_json::to_writer(file, &IdentityJson { identity: encoded })?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
pub struct IdentityJson {
    identity: Vec<u8>,
}
