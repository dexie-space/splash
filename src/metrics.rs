use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use warp::Filter;

#[derive(Clone, Debug)]
pub struct Metrics {
    peers: Arc<AtomicUsize>,
    offers_broadcasted: Arc<AtomicUsize>,
    offers_received: Arc<AtomicUsize>,
    total_connections: Arc<AtomicUsize>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            peers: Arc::new(AtomicUsize::new(0)),
            offers_broadcasted: Arc::new(AtomicUsize::new(0)),
            offers_received: Arc::new(AtomicUsize::new(0)),
            total_connections: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn increment_connections(&self) {
        self.peers.fetch_add(1, Ordering::SeqCst);
        self.total_connections.fetch_add(1, Ordering::SeqCst);
    }

    pub fn decrement_connections(&self) {
        self.peers.fetch_sub(1, Ordering::SeqCst);
    }

    pub fn increment_offers_received(&self) {
        self.offers_received.fetch_add(1, Ordering::SeqCst);
    }

    pub fn increment_offers_broadcasted(&self) {
        self.offers_broadcasted.fetch_add(1, Ordering::SeqCst);
    }

    pub fn get_metrics(&self) -> serde_json::Value {
        json!({
            "peers": self.peers.load(Ordering::SeqCst),
            "offers_broadcasted": self.offers_broadcasted.load(Ordering::SeqCst),
            "offers_received": self.offers_received.load(Ordering::SeqCst),
            "total_connections": self.total_connections.load(Ordering::SeqCst),
        })
    }
}

pub fn metrics_filter(
    metrics: Metrics,
) -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
    warp::get().map(move || warp::reply::json(&metrics.get_metrics()))
}
