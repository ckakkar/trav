use std::net::SocketAddr;
use std::collections::HashMap;

/// A simple XOR-distance Kademlia Routing Table representation.
/// For Phase 2, we keep a simplified flat list of known good nodes avoiding full K-bucket trees.
pub struct RoutingTable {
    pub our_id: [u8; 20],
    pub nodes: HashMap<[u8; 20], SocketAddr>,
}

impl RoutingTable {
    pub fn new(our_id: [u8; 20]) -> Self {
        Self {
            our_id,
            nodes: HashMap::new(),
        }
    }

    /// Add a node to our routing table if it doesn't already exist.
    pub fn add_node(&mut self, id: [u8; 20], addr: SocketAddr) {
        if id != self.our_id {
            self.nodes.insert(id, addr);
        }
    }

    /// Returns the closest 8 nodes to a target ID by sorting via XOR distance.
    pub fn closest_nodes(&self, target: &[u8; 20]) -> Vec<([u8; 20], SocketAddr)> {
        let mut distances: Vec<_> = self.nodes.iter().map(|(id, addr)| {
            let dist = xor_distance(target, id);
            (dist, *id, *addr)
        }).collect();

        // Sort by distance ascending
        distances.sort_by(|a, b| a.0.cmp(&b.0));

        distances.into_iter().take(8).map(|(_, id, addr)| (id, addr)).collect()
    }
}

fn xor_distance(a: &[u8; 20], b: &[u8; 20]) -> [u8; 20] {
    let mut res = [0u8; 20];
    for i in 0..20 {
        res[i] = a[i] ^ b[i];
    }
    res
}
