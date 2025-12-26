//! Topology-aware shard placement for CyxCloud
//!
//! Implements placement strategies that consider:
//! - Datacenter distribution (no more than N shards per DC)
//! - Rack awareness (spread across racks)
//! - Geographic proximity (for latency optimization)
//! - Node capacity and utilization

use crate::models::Node;
use std::collections::{HashMap, HashSet};
use tracing::{debug, warn};

/// Placement strategy configuration
#[derive(Debug, Clone)]
pub struct PlacementConfig {
    /// Maximum shards per datacenter
    pub max_shards_per_dc: usize,
    /// Maximum shards per rack
    pub max_shards_per_rack: usize,
    /// Prefer nodes with lower utilization
    pub prefer_low_utilization: bool,
    /// Weight for utilization in scoring (0.0 - 1.0)
    pub utilization_weight: f64,
    /// Weight for geographic proximity in scoring (0.0 - 1.0)
    pub proximity_weight: f64,
    /// Minimum available storage per node (bytes)
    pub min_available_storage: u64,
}

impl Default for PlacementConfig {
    fn default() -> Self {
        Self {
            max_shards_per_dc: 6,
            max_shards_per_rack: 2,
            prefer_low_utilization: true,
            utilization_weight: 0.5,
            proximity_weight: 0.3,
            min_available_storage: 1024 * 1024 * 1024, // 1 GB
        }
    }
}

/// Node with placement metadata
#[derive(Debug, Clone)]
pub struct PlacementNode {
    pub id: String,
    pub grpc_address: String,
    pub datacenter: Option<String>,
    pub rack: Option<i32>,
    pub region: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub storage_total: u64,
    pub storage_used: u64,
    pub bandwidth_mbps: u32,
}

impl PlacementNode {
    /// Create from database Node model
    pub fn from_node(node: &Node) -> Self {
        Self {
            id: node.id.to_string(),
            grpc_address: node.grpc_address.clone(),
            datacenter: node.datacenter.clone(),
            rack: node.rack,
            region: node.region.clone(),
            latitude: node.latitude,
            longitude: node.longitude,
            storage_total: node.storage_total as u64,
            storage_used: node.storage_used as u64,
            bandwidth_mbps: node.bandwidth_mbps as u32,
        }
    }

    /// Get available storage
    pub fn available_storage(&self) -> u64 {
        self.storage_total.saturating_sub(self.storage_used)
    }

    /// Get utilization percentage
    pub fn utilization(&self) -> f64 {
        if self.storage_total == 0 {
            1.0
        } else {
            self.storage_used as f64 / self.storage_total as f64
        }
    }

    /// Calculate distance to another node (haversine formula)
    pub fn distance_km(&self, other: &PlacementNode) -> Option<f64> {
        match (self.latitude, self.longitude, other.latitude, other.longitude) {
            (Some(lat1), Some(lon1), Some(lat2), Some(lon2)) => {
                let r = 6371.0; // Earth's radius in km
                let lat1_rad = lat1.to_radians();
                let lat2_rad = lat2.to_radians();
                let delta_lat = (lat2 - lat1).to_radians();
                let delta_lon = (lon2 - lon1).to_radians();

                let a = (delta_lat / 2.0).sin().powi(2)
                    + lat1_rad.cos() * lat2_rad.cos() * (delta_lon / 2.0).sin().powi(2);
                let c = 2.0 * a.sqrt().asin();

                Some(r * c)
            }
            _ => None,
        }
    }
}

/// Placement decision for a shard
#[derive(Debug, Clone)]
pub struct PlacementDecision {
    /// Shard index
    pub shard_index: usize,
    /// Selected nodes for this shard
    pub nodes: Vec<PlacementNode>,
    /// Placement score (higher is better)
    pub score: f64,
}

/// Topology-aware placement engine
pub struct PlacementEngine {
    config: PlacementConfig,
}

impl PlacementEngine {
    /// Create a new placement engine
    pub fn new(config: PlacementConfig) -> Self {
        Self { config }
    }

    /// Select nodes for placing a set of shards
    ///
    /// Returns a list of placement decisions, one per shard.
    pub fn select_nodes(
        &self,
        available_nodes: &[PlacementNode],
        num_shards: usize,
        replicas_per_shard: usize,
        origin: Option<&PlacementNode>,
    ) -> Vec<PlacementDecision> {
        if available_nodes.is_empty() {
            return Vec::new();
        }

        // Filter nodes with sufficient storage
        let eligible_nodes: Vec<_> = available_nodes
            .iter()
            .filter(|n| n.available_storage() >= self.config.min_available_storage)
            .cloned()
            .collect();

        if eligible_nodes.is_empty() {
            warn!("No nodes with sufficient storage available");
            return Vec::new();
        }

        let mut decisions = Vec::with_capacity(num_shards);
        let mut dc_usage: HashMap<String, usize> = HashMap::new();
        let mut rack_usage: HashMap<(String, i32), usize> = HashMap::new();

        for shard_index in 0..num_shards {
            let selected = self.select_for_shard(
                &eligible_nodes,
                replicas_per_shard,
                origin,
                &dc_usage,
                &rack_usage,
            );

            // Update usage tracking
            for node in &selected {
                if let Some(dc) = &node.datacenter {
                    *dc_usage.entry(dc.clone()).or_default() += 1;
                }
                if let (Some(dc), Some(rack)) = (&node.datacenter, node.rack) {
                    *rack_usage.entry((dc.clone(), rack)).or_default() += 1;
                }
            }

            let score = self.calculate_placement_score(&selected, origin);

            decisions.push(PlacementDecision {
                shard_index,
                nodes: selected,
                score,
            });
        }

        debug!(
            num_shards = num_shards,
            replicas = replicas_per_shard,
            "Placement decisions generated"
        );

        decisions
    }

    /// Select nodes for a single shard
    fn select_for_shard(
        &self,
        nodes: &[PlacementNode],
        count: usize,
        origin: Option<&PlacementNode>,
        dc_usage: &HashMap<String, usize>,
        rack_usage: &HashMap<(String, i32), usize>,
    ) -> Vec<PlacementNode> {
        if count >= nodes.len() {
            return nodes.to_vec();
        }

        // Score each node
        let mut scored_nodes: Vec<(f64, &PlacementNode)> = nodes
            .iter()
            .map(|node| {
                let score = self.score_node(node, origin, dc_usage, rack_usage);
                (score, node)
            })
            .collect();

        // Sort by score descending
        scored_nodes.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Select top N nodes with diversity constraints
        let mut selected = Vec::with_capacity(count);
        let mut selected_dcs: HashMap<String, usize> = HashMap::new();
        let mut selected_racks: HashMap<(String, i32), usize> = HashMap::new();

        for (_, node) in scored_nodes {
            if selected.len() >= count {
                break;
            }

            // Check datacenter constraint
            if let Some(dc) = &node.datacenter {
                let dc_count = selected_dcs.get(dc).copied().unwrap_or(0);
                if dc_count >= self.config.max_shards_per_dc {
                    continue;
                }
            }

            // Check rack constraint
            if let (Some(dc), Some(rack)) = (&node.datacenter, node.rack) {
                let rack_count = selected_racks.get(&(dc.clone(), rack)).copied().unwrap_or(0);
                if rack_count >= self.config.max_shards_per_rack {
                    continue;
                }
            }

            // Add node
            if let Some(dc) = &node.datacenter {
                *selected_dcs.entry(dc.clone()).or_default() += 1;
            }
            if let (Some(dc), Some(rack)) = (&node.datacenter, node.rack) {
                *selected_racks.entry((dc.clone(), rack)).or_default() += 1;
            }

            selected.push(node.clone());
        }

        selected
    }

    /// Score a node for placement
    fn score_node(
        &self,
        node: &PlacementNode,
        origin: Option<&PlacementNode>,
        dc_usage: &HashMap<String, usize>,
        rack_usage: &HashMap<(String, i32), usize>,
    ) -> f64 {
        let mut score = 100.0;

        // Utilization score (lower utilization = higher score)
        if self.config.prefer_low_utilization {
            let util_score = (1.0 - node.utilization()) * 100.0 * self.config.utilization_weight;
            score += util_score;
        }

        // Proximity score (closer to origin = higher score)
        if let Some(origin) = origin {
            if let Some(distance) = node.distance_km(origin) {
                // Normalize distance to 0-100 scale (assuming max relevant distance is 20,000 km)
                let normalized = (1.0 - (distance / 20000.0).min(1.0)) * 100.0;
                score += normalized * self.config.proximity_weight;
            }
        }

        // Datacenter diversity bonus (prefer less-used DCs)
        if let Some(dc) = &node.datacenter {
            let dc_count = dc_usage.get(dc).copied().unwrap_or(0);
            if dc_count == 0 {
                score += 50.0; // Bonus for new datacenter
            } else {
                score -= (dc_count as f64) * 10.0; // Penalty for each existing shard
            }
        }

        // Rack diversity bonus
        if let (Some(dc), Some(rack)) = (&node.datacenter, node.rack) {
            let rack_count = rack_usage.get(&(dc.clone(), rack)).copied().unwrap_or(0);
            if rack_count == 0 {
                score += 25.0; // Bonus for new rack
            } else {
                score -= (rack_count as f64) * 5.0;
            }
        }

        // Bandwidth bonus
        score += (node.bandwidth_mbps as f64) / 100.0;

        score
    }

    /// Calculate overall placement score
    fn calculate_placement_score(
        &self,
        nodes: &[PlacementNode],
        origin: Option<&PlacementNode>,
    ) -> f64 {
        if nodes.is_empty() {
            return 0.0;
        }

        let mut score = 0.0;

        // Datacenter diversity score
        let mut dcs: HashSet<String> = HashSet::new();
        for node in nodes {
            if let Some(dc) = &node.datacenter {
                dcs.insert(dc.clone());
            }
        }
        score += (dcs.len() as f64) * 20.0;

        // Average utilization (lower is better)
        let avg_util: f64 = nodes.iter().map(|n| n.utilization()).sum::<f64>() / nodes.len() as f64;
        score += (1.0 - avg_util) * 50.0;

        // Average proximity to origin
        if let Some(origin) = origin {
            let distances: Vec<f64> = nodes
                .iter()
                .filter_map(|n| n.distance_km(origin))
                .collect();
            if !distances.is_empty() {
                let avg_distance = distances.iter().sum::<f64>() / distances.len() as f64;
                score += (1.0 - (avg_distance / 20000.0).min(1.0)) * 30.0;
            }
        }

        score
    }

    /// Rebalance: find nodes to move shards from/to
    pub fn suggest_rebalance(
        &self,
        nodes: &[PlacementNode],
        target_utilization: f64,
    ) -> Vec<RebalanceSuggestion> {
        let mut suggestions = Vec::new();

        // Find overutilized nodes (source)
        let overutilized: Vec<_> = nodes
            .iter()
            .filter(|n| n.utilization() > target_utilization + 0.1)
            .collect();

        // Find underutilized nodes (target)
        let underutilized: Vec<_> = nodes
            .iter()
            .filter(|n| n.utilization() < target_utilization - 0.1)
            .collect();

        for source in &overutilized {
            for target in &underutilized {
                // Skip if same datacenter (for diversity)
                if source.datacenter == target.datacenter {
                    continue;
                }

                suggestions.push(RebalanceSuggestion {
                    source_node: source.id.clone(),
                    target_node: target.id.clone(),
                    priority: (source.utilization() - target.utilization()) * 100.0,
                });
            }
        }

        // Sort by priority descending
        suggestions.sort_by(|a, b| b.priority.partial_cmp(&a.priority).unwrap_or(std::cmp::Ordering::Equal));

        suggestions
    }
}

/// Suggestion for rebalancing data
#[derive(Debug, Clone)]
pub struct RebalanceSuggestion {
    pub source_node: String,
    pub target_node: String,
    pub priority: f64,
}

/// Geographic region grouping
pub struct RegionGrouper;

impl RegionGrouper {
    /// Group nodes by region
    pub fn group_by_region(nodes: &[PlacementNode]) -> HashMap<String, Vec<PlacementNode>> {
        let mut groups: HashMap<String, Vec<PlacementNode>> = HashMap::new();

        for node in nodes {
            let region = node.region.clone().unwrap_or_else(|| "unknown".to_string());
            groups.entry(region).or_default().push(node.clone());
        }

        groups
    }

    /// Get nodes closest to a geographic point
    pub fn nearest_nodes(
        nodes: &[PlacementNode],
        lat: f64,
        lon: f64,
        count: usize,
    ) -> Vec<PlacementNode> {
        let reference = PlacementNode {
            id: String::new(),
            grpc_address: String::new(),
            datacenter: None,
            rack: None,
            region: None,
            latitude: Some(lat),
            longitude: Some(lon),
            storage_total: 0,
            storage_used: 0,
            bandwidth_mbps: 0,
        };

        let mut with_distance: Vec<_> = nodes
            .iter()
            .filter_map(|n| {
                n.distance_km(&reference).map(|d| (d, n.clone()))
            })
            .collect();

        with_distance.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        with_distance.into_iter().take(count).map(|(_, n)| n).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_node(id: &str, dc: &str, rack: i32, util: f64) -> PlacementNode {
        let total = 10_000_000_000u64; // 10 GB to exceed min_available_storage requirement
        PlacementNode {
            id: id.to_string(),
            grpc_address: format!("{}:50051", id),
            datacenter: Some(dc.to_string()),
            rack: Some(rack),
            region: Some("test-region".to_string()),
            latitude: Some(40.0),
            longitude: Some(-74.0),
            storage_total: total,
            storage_used: (total as f64 * util) as u64,
            bandwidth_mbps: 1000,
        }
    }

    #[test]
    fn test_placement_config_default() {
        let config = PlacementConfig::default();
        assert_eq!(config.max_shards_per_dc, 6);
        assert_eq!(config.max_shards_per_rack, 2);
    }

    #[test]
    fn test_placement_node_utilization() {
        let node = make_test_node("n1", "dc1", 1, 0.5);
        assert!((node.utilization() - 0.5).abs() < 0.01);
        assert_eq!(node.available_storage(), 5_000_000_000); // 50% of 10 GB
    }

    #[test]
    fn test_placement_engine_selects_diverse_nodes() {
        let engine = PlacementEngine::new(PlacementConfig::default());

        let nodes = vec![
            make_test_node("n1", "dc1", 1, 0.1),
            make_test_node("n2", "dc1", 2, 0.2),
            make_test_node("n3", "dc2", 1, 0.3),
            make_test_node("n4", "dc2", 2, 0.4),
            make_test_node("n5", "dc3", 1, 0.5),
        ];

        let decisions = engine.select_nodes(&nodes, 2, 3, None);

        assert_eq!(decisions.len(), 2);
        for decision in &decisions {
            assert_eq!(decision.nodes.len(), 3);
        }
    }

    #[test]
    fn test_placement_engine_empty_nodes() {
        let engine = PlacementEngine::new(PlacementConfig::default());
        let decisions = engine.select_nodes(&[], 1, 3, None);
        assert!(decisions.is_empty());
    }

    #[test]
    fn test_rebalance_suggestions() {
        let engine = PlacementEngine::new(PlacementConfig::default());

        let nodes = vec![
            make_test_node("n1", "dc1", 1, 0.9), // Overutilized
            make_test_node("n2", "dc2", 1, 0.1), // Underutilized
            make_test_node("n3", "dc3", 1, 0.5), // Balanced
        ];

        let suggestions = engine.suggest_rebalance(&nodes, 0.5);

        // Should suggest moving from n1 to n2
        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].source_node, "n1");
        assert_eq!(suggestions[0].target_node, "n2");
    }

    #[test]
    fn test_region_grouper() {
        let nodes = vec![
            make_test_node("n1", "dc1", 1, 0.5),
            make_test_node("n2", "dc2", 1, 0.5),
        ];

        let groups = RegionGrouper::group_by_region(&nodes);
        assert_eq!(groups.len(), 1); // All in "test-region"
        assert_eq!(groups.get("test-region").unwrap().len(), 2);
    }
}
