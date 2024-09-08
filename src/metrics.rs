use serde::Serialize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

    pub fn increment_peers(&self) -> usize {
        self.peers.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn decrement_peers(&self) -> usize {
        self.peers.fetch_sub(1, Ordering::SeqCst) - 1
    }

    pub fn increment_offers_received(&self) {
        self.offers_received.fetch_add(1, Ordering::SeqCst);
    }

    pub fn increment_offers_broadcasted(&self) {
        self.offers_broadcasted.fetch_add(1, Ordering::SeqCst);
    }

    pub fn get_metrics(&self) -> MetricsData {
        MetricsData {
            peers: self.peers.load(Ordering::SeqCst),
            offers_broadcasted: self.offers_broadcasted.load(Ordering::SeqCst),
            offers_received: self.offers_received.load(Ordering::SeqCst),
            total_connections: self.total_connections.load(Ordering::SeqCst),
        }
    }
}

#[derive(Serialize)]
pub struct MetricsData {
    pub peers: usize,
    pub offers_broadcasted: usize,
    pub offers_received: usize,
    pub total_connections: usize,
}
