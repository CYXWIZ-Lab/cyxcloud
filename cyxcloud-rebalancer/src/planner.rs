//! Repair Planner
//!
//! Creates repair plans from detected chunk issues.
//! Optimizes for:
//! - Network efficiency (prefer nearby nodes)
//! - Load balancing (spread repairs across nodes)
//! - Priority (critical issues first)

use std::collections::{HashMap, HashSet};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, instrument, warn};

use crate::detector::{ChunkHealth, ChunkIssue};

/// Planner errors
#[derive(Error, Debug)]
pub enum PlannerError {
    #[error("No source nodes available for chunk")]
    NoSourceNodes,

    #[error("No target nodes available")]
    NoTargetNodes,

    #[error("Insufficient healthy nodes: have {have}, need {need}")]
    InsufficientNodes { have: usize, need: usize },

    #[error("Planning error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, PlannerError>;

/// A single repair task
#[derive(Debug, Clone)]
pub struct RepairTask {
    /// Unique task ID
    pub task_id: String,
    /// Chunk to repair
    pub chunk_id: Vec<u8>,
    /// Node to read from
    pub source_node: String,
    /// Nodes to write to
    pub target_nodes: Vec<String>,
    /// Chunk size in bytes
    pub chunk_size: u64,
    /// Priority (higher = more urgent)
    pub priority: u32,
    /// Original issue that triggered this repair
    pub issue: ChunkIssue,
}

impl RepairTask {
    /// Estimate bandwidth needed in bytes
    pub fn estimated_bandwidth(&self) -> u64 {
        self.chunk_size * self.target_nodes.len() as u64
    }
}

/// Repair plan containing multiple tasks
#[derive(Debug, Default)]
pub struct RepairPlan {
    /// Ordered list of repair tasks
    pub tasks: Vec<RepairTask>,
    /// Total bytes to transfer
    pub total_bytes: u64,
    /// Estimated duration
    pub estimated_duration: Duration,
    /// Nodes involved as sources
    pub source_nodes: HashSet<String>,
    /// Nodes involved as targets
    pub target_nodes: HashSet<String>,
}

impl RepairPlan {
    /// Add a task to the plan
    pub fn add_task(&mut self, task: RepairTask) {
        self.total_bytes += task.estimated_bandwidth();
        self.source_nodes.insert(task.source_node.clone());

        for target in &task.target_nodes {
            self.target_nodes.insert(target.clone());
        }

        self.tasks.push(task);
    }

    /// Get tasks for a specific source node
    pub fn tasks_for_source(&self, node_id: &str) -> Vec<&RepairTask> {
        self.tasks
            .iter()
            .filter(|t| t.source_node == node_id)
            .collect()
    }

    /// Get tasks for a specific target node
    pub fn tasks_for_target(&self, node_id: &str) -> Vec<&RepairTask> {
        self.tasks
            .iter()
            .filter(|t| t.target_nodes.contains(&node_id.to_string()))
            .collect()
    }

    /// Summary of the plan
    pub fn summary(&self) -> String {
        format!(
            "{} tasks, {} bytes to transfer, {} source nodes, {} target nodes",
            self.tasks.len(),
            self.total_bytes,
            self.source_nodes.len(),
            self.target_nodes.len()
        )
    }
}

/// Node information for planning
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub id: String,
    pub address: String,
    /// Available storage in bytes
    pub available_storage: u64,
    /// Current load (0.0 - 1.0)
    pub load: f64,
    /// Datacenter/region for locality
    pub datacenter: Option<String>,
    /// Is node healthy?
    pub is_healthy: bool,
}

/// Planner configuration
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    /// Target replication factor
    pub replication_factor: usize,
    /// Maximum tasks per plan
    pub max_tasks: usize,
    /// Maximum bytes per plan
    pub max_bytes: u64,
    /// Prefer same-datacenter transfers
    pub prefer_local: bool,
    /// Maximum load on a single node (0.0 - 1.0)
    pub max_node_load: f64,
    /// Rate limit per node (bytes/second)
    pub node_rate_limit: u64,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            replication_factor: 3,
            max_tasks: 100,
            max_bytes: 10 * 1024 * 1024 * 1024, // 10 GB per plan
            prefer_local: true,
            max_node_load: 0.8,
            node_rate_limit: 100 * 1024 * 1024, // 100 MB/s
        }
    }
}

/// Repair planner
pub struct Planner {
    config: PlannerConfig,
    /// Track pending load per node
    pending_load: HashMap<String, u64>,
    /// Task ID counter
    task_counter: u64,
}

impl Planner {
    /// Create a new planner
    pub fn new(config: PlannerConfig) -> Self {
        Self {
            config,
            pending_load: HashMap::new(),
            task_counter: 0,
        }
    }

    /// Create a repair plan from issues
    #[instrument(skip(self, issues, nodes))]
    pub fn create_plan(&mut self, issues: &[ChunkIssue], nodes: &[NodeInfo]) -> Result<RepairPlan> {
        let mut plan = RepairPlan::default();

        // Filter to only healthy nodes
        let healthy_nodes: Vec<_> = nodes.iter().filter(|n| n.is_healthy).collect();

        if healthy_nodes.is_empty() {
            return Err(PlannerError::NoTargetNodes);
        }

        info!(
            issues = issues.len(),
            healthy_nodes = healthy_nodes.len(),
            "Creating repair plan"
        );

        // Sort issues by priority (highest first)
        let mut sorted_issues: Vec<_> = issues.iter().collect();
        sorted_issues.sort_by(|a, b| b.priority.cmp(&a.priority));

        // Reset pending load tracking
        self.pending_load.clear();

        for issue in sorted_issues {
            // Check plan limits
            if plan.tasks.len() >= self.config.max_tasks {
                debug!("Reached max tasks limit");
                break;
            }
            if plan.total_bytes >= self.config.max_bytes {
                debug!("Reached max bytes limit");
                break;
            }

            // Try to create a repair task for this issue
            match self.plan_repair(issue, &healthy_nodes) {
                Ok(task) => {
                    // Update pending load
                    *self
                        .pending_load
                        .entry(task.source_node.clone())
                        .or_default() += task.chunk_size;
                    for target in &task.target_nodes {
                        *self.pending_load.entry(target.clone()).or_default() += task.chunk_size;
                    }

                    plan.add_task(task);
                }
                Err(e) => {
                    warn!(
                        chunk_id = hex::encode(&issue.chunk_id),
                        error = %e,
                        "Could not plan repair for chunk"
                    );
                }
            }
        }

        // Estimate duration based on rate limits
        plan.estimated_duration = self.estimate_duration(&plan);

        info!(summary = %plan.summary(), "Repair plan created");

        Ok(plan)
    }

    /// Plan repair for a single chunk
    fn plan_repair(&mut self, issue: &ChunkIssue, nodes: &[&NodeInfo]) -> Result<RepairTask> {
        match &issue.health {
            ChunkHealth::UnderReplicated { current, target } => {
                self.plan_under_replicated(issue, nodes, *current, *target)
            }
            ChunkHealth::Critical => {
                // For critical chunks, we may not have any source
                if issue.current_nodes.is_empty() {
                    return Err(PlannerError::NoSourceNodes);
                }
                self.plan_under_replicated(issue, nodes, 0, self.config.replication_factor)
            }
            ChunkHealth::OverReplicated {
                current: _,
                target: _,
            } => {
                // For over-replicated, we don't need repair, just cleanup
                // This would be handled differently (delete extra copies)
                Err(PlannerError::Internal(
                    "Over-replicated chunks don't need repair".to_string(),
                ))
            }
            _ => Err(PlannerError::Internal(format!(
                "Unhandled health status: {:?}",
                issue.health
            ))),
        }
    }

    /// Plan repair for under-replicated chunk
    fn plan_under_replicated(
        &mut self,
        issue: &ChunkIssue,
        nodes: &[&NodeInfo],
        current: usize,
        target: usize,
    ) -> Result<RepairTask> {
        // Find source node (must be in current_nodes and healthy)
        let source = self.select_source_node(issue, nodes)?;

        // Calculate how many replicas we need to create
        let replicas_needed = target.saturating_sub(current);
        if replicas_needed == 0 {
            return Err(PlannerError::Internal("No replicas needed".to_string()));
        }

        // Select target nodes
        let targets = self.select_target_nodes(issue, nodes, &source, replicas_needed)?;

        // Generate task ID
        self.task_counter += 1;
        let task_id = format!("repair-{}", self.task_counter);

        Ok(RepairTask {
            task_id,
            chunk_id: issue.chunk_id.clone(),
            source_node: source,
            target_nodes: targets,
            chunk_size: 1024 * 1024, // Default 1MB, should come from metadata
            priority: issue.priority,
            issue: issue.clone(),
        })
    }

    /// Select best source node for reading
    fn select_source_node(&self, issue: &ChunkIssue, nodes: &[&NodeInfo]) -> Result<String> {
        let healthy_sources: Vec<_> = nodes
            .iter()
            .filter(|n| issue.current_nodes.contains(&n.id))
            .collect();

        if healthy_sources.is_empty() {
            return Err(PlannerError::NoSourceNodes);
        }

        // Prefer node with lowest load
        let best = healthy_sources
            .iter()
            .min_by(|a, b| {
                let load_a = self.get_node_load(&a.id, a.load);
                let load_b = self.get_node_load(&b.id, b.load);
                load_a.partial_cmp(&load_b).unwrap()
            })
            .unwrap();

        Ok(best.id.clone())
    }

    /// Select target nodes for writing
    fn select_target_nodes(
        &self,
        issue: &ChunkIssue,
        nodes: &[&NodeInfo],
        source: &str,
        count: usize,
    ) -> Result<Vec<String>> {
        let current_set: HashSet<_> = issue.current_nodes.iter().cloned().collect();

        // Get source datacenter for locality preference
        let source_dc = nodes
            .iter()
            .find(|n| n.id == source)
            .and_then(|n| n.datacenter.clone());

        // Filter candidates: not already holding chunk, has space, not overloaded
        let mut candidates: Vec<_> = nodes
            .iter()
            .filter(|n| {
                !current_set.contains(&n.id)
                    && n.id != source
                    && n.available_storage >= 1024 * 1024 // At least 1MB free
                    && self.get_node_load(&n.id, n.load) < self.config.max_node_load
            })
            .collect();

        if candidates.len() < count {
            return Err(PlannerError::InsufficientNodes {
                have: candidates.len(),
                need: count,
            });
        }

        // Score candidates
        candidates.sort_by(|a, b| {
            let score_a = self.score_target(a, &source_dc);
            let score_b = self.score_target(b, &source_dc);
            score_b.partial_cmp(&score_a).unwrap() // Higher score is better
        });

        // Take top N
        let targets: Vec<String> = candidates
            .iter()
            .take(count)
            .map(|n| n.id.clone())
            .collect();

        Ok(targets)
    }

    /// Score a target node (higher is better)
    fn score_target(&self, node: &NodeInfo, source_dc: &Option<String>) -> f64 {
        let mut score = 1.0;

        // Prefer same datacenter
        if self.config.prefer_local {
            if let (Some(src_dc), Some(node_dc)) = (source_dc, &node.datacenter) {
                if src_dc == node_dc {
                    score += 0.5;
                }
            }
        }

        // Prefer nodes with more free space
        let storage_score =
            (node.available_storage as f64 / (100u64 * 1024 * 1024 * 1024) as f64).min(1.0);
        score += storage_score * 0.3;

        // Prefer nodes with lower load
        let load = self.get_node_load(&node.id, node.load);
        score += (1.0 - load) * 0.2;

        score
    }

    /// Get effective node load including pending operations
    fn get_node_load(&self, node_id: &str, base_load: f64) -> f64 {
        let pending = *self.pending_load.get(node_id).unwrap_or(&0) as f64;
        let pending_load = pending / (10.0 * 1024.0 * 1024.0 * 1024.0); // Normalize to 10GB
        (base_load + pending_load).min(1.0)
    }

    /// Estimate plan execution duration
    fn estimate_duration(&self, plan: &RepairPlan) -> Duration {
        if plan.tasks.is_empty() || plan.source_nodes.is_empty() {
            return Duration::ZERO;
        }

        // Assume parallelism across nodes
        let max_per_node = plan
            .source_nodes
            .iter()
            .map(|n| {
                plan.tasks_for_source(n)
                    .iter()
                    .map(|t| t.chunk_size)
                    .sum::<u64>()
            })
            .max()
            .unwrap_or(0);

        let seconds = max_per_node / self.config.node_rate_limit;
        Duration::from_secs(seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn make_issue(chunk_id: u8, nodes: Vec<&str>, priority: u32) -> ChunkIssue {
        ChunkIssue {
            chunk_id: vec![chunk_id],
            health: ChunkHealth::UnderReplicated {
                current: nodes.len(),
                target: 3,
            },
            current_nodes: nodes.iter().map(|s| s.to_string()).collect(),
            file_id: None,
            priority,
            detected_at: Instant::now(),
        }
    }

    fn make_node(id: &str, dc: &str, load: f64) -> NodeInfo {
        NodeInfo {
            id: id.to_string(),
            address: format!("{}:50051", id),
            available_storage: 100 * 1024 * 1024 * 1024, // 100 GB
            load,
            datacenter: Some(dc.to_string()),
            is_healthy: true,
        }
    }

    #[test]
    fn test_planner_config_default() {
        let config = PlannerConfig::default();
        assert_eq!(config.replication_factor, 3);
        assert_eq!(config.max_tasks, 100);
        assert!(config.prefer_local);
    }

    #[test]
    fn test_create_plan_basic() {
        let mut planner = Planner::new(PlannerConfig::default());

        let issues = vec![make_issue(1, vec!["n1"], 800)];

        let nodes = vec![
            make_node("n1", "dc1", 0.1),
            make_node("n2", "dc1", 0.2),
            make_node("n3", "dc2", 0.3),
            make_node("n4", "dc2", 0.4),
        ];

        let plan = planner.create_plan(&issues, &nodes).unwrap();

        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].source_node, "n1");
        assert_eq!(plan.tasks[0].target_nodes.len(), 2); // Need 2 more replicas
    }

    #[test]
    fn test_create_plan_priority_order() {
        let mut planner = Planner::new(PlannerConfig::default());

        let issues = vec![
            make_issue(1, vec!["n1"], 500),
            make_issue(2, vec!["n1"], 800), // Higher priority
        ];

        let nodes = vec![
            make_node("n1", "dc1", 0.1),
            make_node("n2", "dc1", 0.2),
            make_node("n3", "dc2", 0.3),
            make_node("n4", "dc2", 0.4),
        ];

        let plan = planner.create_plan(&issues, &nodes).unwrap();

        assert_eq!(plan.tasks.len(), 2);
        // Higher priority task should come first
        assert_eq!(plan.tasks[0].priority, 800);
        assert_eq!(plan.tasks[1].priority, 500);
    }

    #[test]
    fn test_repair_plan_summary() {
        let plan = RepairPlan {
            tasks: vec![],
            total_bytes: 1024 * 1024,
            estimated_duration: Duration::from_secs(10),
            source_nodes: ["n1".to_string()].into_iter().collect(),
            target_nodes: ["n2".to_string(), "n3".to_string()].into_iter().collect(),
        };

        let summary = plan.summary();
        assert!(summary.contains("1 source nodes"));
        assert!(summary.contains("2 target nodes"));
    }
}
