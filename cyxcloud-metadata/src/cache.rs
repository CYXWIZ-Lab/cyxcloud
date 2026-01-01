//! Redis caching layer for CyxCloud metadata
//!
//! Provides caching for hot paths like chunk locations and node lookups.

use redis::aio::MultiplexedConnection;
use redis::{AsyncCommands, Client};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, warn};

/// Cache error types
#[derive(Error, Debug)]
pub enum CacheError {
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Cache miss")]
    Miss,
}

pub type Result<T> = std::result::Result<T, CacheError>;

/// Cache configuration
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Redis connection URL
    pub url: String,
    /// Default TTL for cached items
    pub default_ttl: Duration,
    /// TTL for chunk location cache
    pub chunk_location_ttl: Duration,
    /// TTL for node info cache
    pub node_info_ttl: Duration,
    /// Key prefix
    pub prefix: String,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            url: "redis://localhost:6379".to_string(),
            default_ttl: Duration::from_secs(300), // 5 minutes
            chunk_location_ttl: Duration::from_secs(60), // 1 minute
            node_info_ttl: Duration::from_secs(120), // 2 minutes
            prefix: "cyxcloud".to_string(),
        }
    }
}

/// Redis cache client
#[derive(Clone)]
pub struct Cache {
    conn: MultiplexedConnection,
    config: CacheConfig,
}

impl Cache {
    /// Create a new cache connection
    pub async fn new(config: CacheConfig) -> Result<Self> {
        let client = Client::open(config.url.as_str())?;
        let conn = client.get_multiplexed_async_connection().await?;
        info!("Connected to Redis cache");
        Ok(Self { conn, config })
    }

    /// Build a cache key with prefix
    fn key(&self, parts: &[&str]) -> String {
        let mut key = self.config.prefix.clone();
        for part in parts {
            key.push(':');
            key.push_str(part);
        }
        key
    }

    /// Set a value with TTL
    pub async fn set<T: Serialize>(&self, key: &str, value: &T, ttl: Duration) -> Result<()> {
        let json = serde_json::to_string(value)?;
        let mut conn = self.conn.clone();
        conn.set_ex::<_, _, ()>(key, json, ttl.as_secs()).await?;
        debug!(key = %key, ttl_secs = ttl.as_secs(), "Cache set");
        Ok(())
    }

    /// Get a value
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<T> {
        let mut conn = self.conn.clone();
        let json: Option<String> = conn.get(key).await?;
        match json {
            Some(json) => {
                let value = serde_json::from_str(&json)?;
                debug!(key = %key, "Cache hit");
                Ok(value)
            }
            None => {
                debug!(key = %key, "Cache miss");
                Err(CacheError::Miss)
            }
        }
    }

    /// Delete a key
    pub async fn delete(&self, key: &str) -> Result<()> {
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(key).await?;
        debug!(key = %key, "Cache delete");
        Ok(())
    }

    /// Check if a key exists
    pub async fn exists(&self, key: &str) -> Result<bool> {
        let mut conn = self.conn.clone();
        let exists: bool = conn.exists(key).await?;
        Ok(exists)
    }

    /// Set multiple values atomically
    pub async fn mset<T: Serialize>(&self, items: &[(&str, &T)], ttl: Duration) -> Result<()> {
        let mut conn = self.conn.clone();
        let mut pipe = redis::pipe();

        for (key, value) in items {
            let json = serde_json::to_string(value)?;
            pipe.set_ex(*key, json, ttl.as_secs());
        }

        pipe.query_async::<_, ()>(&mut conn).await?;
        debug!(count = items.len(), "Cache mset");
        Ok(())
    }

    /// Get multiple values
    pub async fn mget<T: DeserializeOwned>(&self, keys: &[&str]) -> Result<Vec<Option<T>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }

        let mut conn = self.conn.clone();
        let values: Vec<Option<String>> = conn.mget(keys).await?;

        let mut results = Vec::with_capacity(values.len());
        for value in values {
            match value {
                Some(json) => {
                    let parsed: T = serde_json::from_str(&json)?;
                    results.push(Some(parsed));
                }
                None => results.push(None),
            }
        }

        Ok(results)
    }

    // =========================================================================
    // CHUNK LOCATION CACHING
    // =========================================================================

    /// Cache chunk locations
    pub async fn set_chunk_locations(
        &self,
        chunk_id: &[u8],
        node_addresses: &[String],
    ) -> Result<()> {
        let key = self.key(&["chunk", &hex::encode(chunk_id)]);
        self.set(&key, &node_addresses, self.config.chunk_location_ttl)
            .await
    }

    /// Get cached chunk locations
    pub async fn get_chunk_locations(&self, chunk_id: &[u8]) -> Result<Vec<String>> {
        let key = self.key(&["chunk", &hex::encode(chunk_id)]);
        self.get(&key).await
    }

    /// Invalidate chunk location cache
    pub async fn invalidate_chunk_locations(&self, chunk_id: &[u8]) -> Result<()> {
        let key = self.key(&["chunk", &hex::encode(chunk_id)]);
        self.delete(&key).await
    }

    // =========================================================================
    // NODE CACHING
    // =========================================================================

    /// Cache node info
    pub async fn set_node_info<T: Serialize>(&self, node_id: &str, info: &T) -> Result<()> {
        let key = self.key(&["node", node_id]);
        self.set(&key, info, self.config.node_info_ttl).await
    }

    /// Get cached node info
    pub async fn get_node_info<T: DeserializeOwned>(&self, node_id: &str) -> Result<T> {
        let key = self.key(&["node", node_id]);
        self.get(&key).await
    }

    /// Invalidate node cache
    pub async fn invalidate_node(&self, node_id: &str) -> Result<()> {
        let key = self.key(&["node", node_id]);
        self.delete(&key).await
    }

    /// Cache online nodes list
    pub async fn set_online_nodes<T: Serialize>(&self, nodes: &[T]) -> Result<()> {
        let key = self.key(&["nodes", "online"]);
        self.set(&key, &nodes, self.config.node_info_ttl).await
    }

    /// Get cached online nodes list
    pub async fn get_online_nodes<T: DeserializeOwned>(&self) -> Result<Vec<T>> {
        let key = self.key(&["nodes", "online"]);
        self.get(&key).await
    }

    // =========================================================================
    // FILE METADATA CACHING
    // =========================================================================

    /// Cache file metadata
    pub async fn set_file<T: Serialize>(&self, file_id: &str, file: &T) -> Result<()> {
        let key = self.key(&["file", file_id]);
        self.set(&key, file, self.config.default_ttl).await
    }

    /// Get cached file metadata
    pub async fn get_file<T: DeserializeOwned>(&self, file_id: &str) -> Result<T> {
        let key = self.key(&["file", file_id]);
        self.get(&key).await
    }

    /// Cache file by path
    pub async fn set_file_by_path<T: Serialize>(&self, path: &str, file: &T) -> Result<()> {
        let key = self.key(&["file_path", path]);
        self.set(&key, file, self.config.default_ttl).await
    }

    /// Get cached file by path
    pub async fn get_file_by_path<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let key = self.key(&["file_path", path]);
        self.get(&key).await
    }

    /// Invalidate file cache
    pub async fn invalidate_file(&self, file_id: &str, path: Option<&str>) -> Result<()> {
        let key = self.key(&["file", file_id]);
        self.delete(&key).await?;

        if let Some(path) = path {
            let path_key = self.key(&["file_path", path]);
            self.delete(&path_key).await?;
        }

        Ok(())
    }

    // =========================================================================
    // DISTRIBUTED LOCKING
    // =========================================================================

    /// Acquire a distributed lock
    pub async fn acquire_lock(&self, name: &str, ttl: Duration) -> Result<bool> {
        let key = self.key(&["lock", name]);
        let mut conn = self.conn.clone();

        // Use SET NX EX for atomic lock acquisition
        let result: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg("locked")
            .arg("NX")
            .arg("EX")
            .arg(ttl.as_secs())
            .query_async(&mut conn)
            .await?;

        let acquired = result.is_some();
        debug!(lock = %name, acquired = acquired, "Lock acquisition attempt");
        Ok(acquired)
    }

    /// Release a distributed lock
    pub async fn release_lock(&self, name: &str) -> Result<()> {
        let key = self.key(&["lock", name]);
        self.delete(&key).await
    }

    /// Extend lock TTL
    pub async fn extend_lock(&self, name: &str, ttl: Duration) -> Result<bool> {
        let key = self.key(&["lock", name]);
        let mut conn = self.conn.clone();

        // Only extend if lock exists
        let extended: bool = conn.expire(&key, ttl.as_secs() as i64).await?;
        debug!(lock = %name, extended = extended, "Lock extension attempt");
        Ok(extended)
    }

    // =========================================================================
    // RATE LIMITING
    // =========================================================================

    /// Check and increment rate limit
    pub async fn check_rate_limit(
        &self,
        key_suffix: &str,
        max_requests: u64,
        window: Duration,
    ) -> Result<(bool, u64)> {
        let key = self.key(&["ratelimit", key_suffix]);
        let mut conn = self.conn.clone();

        // Increment counter
        let count: u64 = conn.incr(&key, 1u64).await?;

        // Set expiry on first request
        if count == 1 {
            conn.expire::<_, ()>(&key, window.as_secs() as i64).await?;
        }

        let allowed = count <= max_requests;
        Ok((allowed, count))
    }

    // =========================================================================
    // STATISTICS
    // =========================================================================

    /// Get cache statistics
    pub async fn get_stats(&self) -> Result<CacheStats> {
        let mut conn = self.conn.clone();
        let info: String = redis::cmd("INFO")
            .arg("stats")
            .query_async(&mut conn)
            .await?;

        // Parse basic stats from INFO output
        let mut stats = CacheStats::default();

        for line in info.lines() {
            if let Some((key, value)) = line.split_once(':') {
                match key {
                    "keyspace_hits" => stats.hits = value.parse().unwrap_or(0),
                    "keyspace_misses" => stats.misses = value.parse().unwrap_or(0),
                    "total_commands_processed" => stats.commands = value.parse().unwrap_or(0),
                    _ => {}
                }
            }
        }

        Ok(stats)
    }
}

/// Cache statistics
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub commands: u64,
}

impl CacheStats {
    /// Calculate hit ratio
    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// Optional cache wrapper - allows graceful degradation if Redis is unavailable
pub struct OptionalCache {
    cache: Option<Cache>,
}

impl OptionalCache {
    /// Create with cache
    pub fn new(cache: Cache) -> Self {
        Self { cache: Some(cache) }
    }

    /// Create without cache (no-op)
    pub fn none() -> Self {
        Self { cache: None }
    }

    /// Try to get from cache, return None on miss or error
    pub async fn try_get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        match &self.cache {
            Some(cache) => cache.get(key).await.ok(),
            None => None,
        }
    }

    /// Try to set in cache, ignore errors
    pub async fn try_set<T: Serialize>(&self, key: &str, value: &T, ttl: Duration) {
        if let Some(cache) = &self.cache {
            if let Err(e) = cache.set(key, value, ttl).await {
                warn!(key = %key, error = %e, "Cache set failed");
            }
        }
    }

    /// Try to delete from cache, ignore errors
    pub async fn try_delete(&self, key: &str) {
        if let Some(cache) = &self.cache {
            if let Err(e) = cache.delete(key).await {
                warn!(key = %key, error = %e, "Cache delete failed");
            }
        }
    }

    /// Check if cache is available
    pub fn is_available(&self) -> bool {
        self.cache.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_config_default() {
        let config = CacheConfig::default();
        assert_eq!(config.default_ttl, Duration::from_secs(300));
        assert_eq!(config.prefix, "cyxcloud");
    }

    #[test]
    fn test_cache_stats() {
        let stats = CacheStats {
            hits: 80,
            misses: 20,
            commands: 100,
        };
        assert!((stats.hit_ratio() - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_cache_stats_zero() {
        let stats = CacheStats::default();
        assert_eq!(stats.hit_ratio(), 0.0);
    }
}
