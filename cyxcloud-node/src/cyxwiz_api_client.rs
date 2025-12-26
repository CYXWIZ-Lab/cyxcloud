//! CyxWiz API Client
//!
//! HTTP client for communicating with the CyxWiz REST API server.
//! Handles machine registration, heartbeats, and wallet queries.

use crate::config::CyxWizApiSettings;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, error, info, warn};

/// API client errors
#[derive(Error, Debug)]
pub enum ApiError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    #[error("API error: {0}")]
    ApiError(String),

    #[error("Not configured: {0}")]
    NotConfigured(String),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),
}

// ============ Request/Response Types ============

/// Hardware info for registration
#[derive(Debug, Clone, Serialize)]
pub struct DetectedHardware {
    pub cpus: Vec<CpuInfo>,
    pub gpus: Vec<GpuInfo>,
    pub ram_total_mb: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuInfo {
    pub name: String,
    pub cores: u32,
    pub threads: u32,
    pub frequency_mhz: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuInfo {
    pub device_id: u32,
    pub name: String,
    pub vendor: String,
    pub vram_mb: u64,
    pub cuda_version: Option<String>,
    pub driver_version: Option<String>,
}

/// Allocation request for compute units
#[derive(Debug, Clone, Serialize)]
pub struct AllocationRequest {
    pub device_type: String,
    pub device_id: u32,
    pub is_enabled: bool,
    pub vram_allocated_mb: Option<u64>,
    pub vram_reserved_mb: Option<u64>,
    pub cores_allocated: Option<u32>,
    pub cores_reserved: Option<u32>,
    pub priority: Option<String>,
    pub max_power_percent: Option<u8>,
}

/// Machine registration request
#[derive(Debug, Serialize)]
pub struct RegisterMachineRequest {
    pub owner_id: String,
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub hardware: Option<DetectedHardware>,
    pub allocations: Option<Vec<AllocationRequest>>,
}

/// Machine registration response
#[derive(Debug, Deserialize)]
pub struct RegisterMachineResponse {
    pub machine: MachineInfo,
    pub compute_units: Vec<ComputeUnitInfo>,
    pub api_key: String,
    pub node: LegacyNodeInfo,
}

#[derive(Debug, Deserialize)]
pub struct MachineInfo {
    pub id: String,
    pub machine_id: String,
    pub owner_id: String,
    pub hostname: String,
    pub os: String,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct ComputeUnitInfo {
    pub id: String,
    pub unit_id: String,
    pub device_type: String,
    pub device_id: u32,
    pub device_name: String,
}

#[derive(Debug, Deserialize)]
pub struct LegacyNodeInfo {
    pub node_id: String,
}

/// Unit status update for heartbeat
#[derive(Debug, Serialize)]
pub struct UnitStatusUpdate {
    pub unit_id: String,
    pub current_load: u8,
    pub status: Option<String>,
}

/// Heartbeat request
#[derive(Debug, Serialize)]
pub struct HeartbeatRequest {
    pub api_key: String,
    pub ip_address: Option<String>,
    pub version: Option<String>,
    pub is_online: bool,
    pub unit_statuses: Option<Vec<UnitStatusUpdate>>,
    pub current_load: Option<u8>,
}

/// Heartbeat response
#[derive(Debug, Deserialize)]
pub struct HeartbeatResponse {
    pub machine: MachineInfo,
    pub compute_units: Vec<ComputeUnitInfo>,
}

/// Wallet balance response
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllBalancesResponse {
    pub spot: f64,
    pub earn: f64,
    pub funding: f64,
    pub futures: f64,
    pub currency: String,
}

/// Error response from API
#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

// ============ Auth Types ============

/// Login request
#[derive(Debug, Serialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Login response
#[derive(Debug, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub user: UserInfo,
}

/// User info from login
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub username: String,
    pub name: Option<String>,
    pub cyx_wallet: Option<CyxWalletInfo>,
    pub external_wallet: Option<String>,
}

/// CyxWallet info
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CyxWalletInfo {
    pub public_key: String,
    pub created_at: Option<String>,
}

/// Saved credentials for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCredentials {
    pub user_id: String,
    pub email: String,
    pub username: String,
    pub auth_token: String,
    pub machine_id: Option<String>,
    pub api_key: Option<String>,
}

// ============ API Client ============

/// Client for CyxWiz REST API
pub struct CyxWizApiClient {
    client: Client,
    base_url: String,
    api_key: Option<String>,
    machine_id: Option<String>,
    auth_token: Option<String>,
    user_id: Option<String>,
}

impl CyxWizApiClient {
    /// Create a new API client from config
    pub fn new(config: &CyxWizApiSettings) -> Result<Self, ApiError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()?;

        Ok(Self {
            client,
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            machine_id: config.machine_id.clone(),
            auth_token: None,
            user_id: config.owner_id.clone(),
        })
    }

    /// Create a new API client with just base URL
    pub fn with_base_url(base_url: &str) -> Result<Self, ApiError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self {
            client,
            base_url: base_url.to_string(),
            api_key: None,
            machine_id: None,
            auth_token: None,
            user_id: None,
        })
    }

    /// Login with email and password
    pub async fn login(&mut self, email: &str, password: &str) -> Result<LoginResponse, ApiError> {
        let url = format!("{}/api/auth/login", self.base_url);

        let request = LoginRequest {
            email: email.to_string(),
            password: password.to_string(),
        };

        info!(email = email, url = %url, "Logging in to CyxWiz API");

        let response = self.client.post(&url).json(&request).send().await?;

        if response.status().is_success() {
            let result: LoginResponse = response.json().await?;

            // Store auth token and user ID
            self.auth_token = Some(result.token.clone());
            self.user_id = Some(result.user.id.clone());

            info!(
                user_id = %result.user.id,
                username = %result.user.username,
                "Login successful"
            );

            Ok(result)
        } else if response.status().as_u16() == 401 {
            Err(ApiError::AuthFailed("Invalid email or password".to_string()))
        } else {
            let error: ErrorResponse = response.json().await.unwrap_or(ErrorResponse {
                error: "Unknown error".to_string(),
            });
            error!(error = %error.error, "Login failed");
            Err(ApiError::ApiError(error.error))
        }
    }

    /// Load saved credentials
    pub fn load_credentials(&mut self, creds: SavedCredentials) {
        self.user_id = Some(creds.user_id);
        self.auth_token = Some(creds.auth_token);
        self.machine_id = creds.machine_id;
        self.api_key = creds.api_key;
    }

    /// Get current credentials for saving
    pub fn get_credentials(&self, email: &str, username: &str) -> Option<SavedCredentials> {
        Some(SavedCredentials {
            user_id: self.user_id.clone()?,
            email: email.to_string(),
            username: username.to_string(),
            auth_token: self.auth_token.clone()?,
            machine_id: self.machine_id.clone(),
            api_key: self.api_key.clone(),
        })
    }

    /// Update API key and machine ID after registration
    pub fn set_credentials(&mut self, api_key: String, machine_id: String) {
        self.api_key = Some(api_key);
        self.machine_id = Some(machine_id);
    }

    /// Get current machine ID
    pub fn machine_id(&self) -> Option<&str> {
        self.machine_id.as_deref()
    }

    /// Get current API key
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Get current user ID
    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }

    /// Check if logged in
    pub fn is_logged_in(&self) -> bool {
        self.auth_token.is_some() && self.user_id.is_some()
    }

    /// Register a new machine with the CyxWiz API
    /// Requires login first (uses stored user_id)
    pub async fn register_machine(
        &mut self,
        hostname: &str,
        os: &str,
        hardware: DetectedHardware,
    ) -> Result<RegisterMachineResponse, ApiError> {
        let owner_id = self
            .user_id
            .as_ref()
            .ok_or_else(|| ApiError::NotConfigured("Not logged in - user_id not set".to_string()))?
            .clone();

        let url = format!("{}/api/machines/register", self.base_url);

        let request = RegisterMachineRequest {
            owner_id: owner_id.clone(),
            hostname: Some(hostname.to_string()),
            os: Some(os.to_string()),
            hardware: Some(hardware),
            allocations: None, // Let API auto-create from hardware
        };

        info!(
            owner_id = %owner_id,
            hostname = hostname,
            url = %url,
            "Registering machine with CyxWiz API"
        );

        let response = self.client.post(&url).json(&request).send().await?;

        if response.status().is_success() {
            let result: RegisterMachineResponse = response.json().await?;

            // Store credentials for future requests
            self.api_key = Some(result.api_key.clone());
            self.machine_id = Some(result.machine.machine_id.clone());

            info!(
                machine_id = %result.machine.machine_id,
                units = result.compute_units.len(),
                "Machine registered successfully"
            );

            Ok(result)
        } else {
            let error: ErrorResponse = response.json().await.unwrap_or(ErrorResponse {
                error: "Unknown error".to_string(),
            });
            error!(error = %error.error, "Machine registration failed");
            Err(ApiError::ApiError(error.error))
        }
    }

    /// Send heartbeat to CyxWiz API
    pub async fn heartbeat(
        &self,
        ip_address: Option<&str>,
        version: Option<&str>,
        is_online: bool,
        current_load: Option<u8>,
    ) -> Result<HeartbeatResponse, ApiError> {
        let machine_id = self
            .machine_id
            .as_ref()
            .ok_or_else(|| ApiError::NotConfigured("Machine ID not set".to_string()))?;

        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| ApiError::NotConfigured("API key not set".to_string()))?;

        let url = format!("{}/api/machines/{}/heartbeat", self.base_url, machine_id);

        let request = HeartbeatRequest {
            api_key: api_key.clone(),
            ip_address: ip_address.map(|s| s.to_string()),
            version: version.map(|s| s.to_string()),
            is_online,
            unit_statuses: None,
            current_load,
        };

        let response = self.client.post(&url).json(&request).send().await?;

        if response.status().is_success() {
            let result: HeartbeatResponse = response.json().await?;
            debug!(
                machine_id = %machine_id,
                status = %result.machine.status,
                "Heartbeat acknowledged"
            );
            Ok(result)
        } else if response.status().as_u16() == 401 {
            warn!(machine_id = %machine_id, "Heartbeat unauthorized - need to re-register");
            Err(ApiError::AuthFailed("Invalid API key".to_string()))
        } else {
            let error: ErrorResponse = response.json().await.unwrap_or(ErrorResponse {
                error: "Unknown error".to_string(),
            });
            Err(ApiError::ApiError(error.error))
        }
    }

    /// Get wallet balances (requires user auth token, not machine API key)
    pub async fn get_balances(&self, auth_token: &str) -> Result<AllBalancesResponse, ApiError> {
        let url = format!("{}/api/wallet/balances", self.base_url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", auth_token))
            .send()
            .await?;

        if response.status().is_success() {
            let result: AllBalancesResponse = response.json().await?;
            Ok(result)
        } else {
            let error: ErrorResponse = response.json().await.unwrap_or(ErrorResponse {
                error: "Unknown error".to_string(),
            });
            Err(ApiError::ApiError(error.error))
        }
    }

    /// Check if machine is registered
    pub fn is_registered(&self) -> bool {
        self.api_key.is_some() && self.machine_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let config = CyxWizApiSettings {
            base_url: "http://localhost:3002".to_string(),
            owner_id: None,
            api_key: None,
            machine_id: None,
            register: true,
            timeout_secs: 10,
        };

        let client = CyxWizApiClient::new(&config);
        assert!(client.is_ok());
    }

    #[test]
    fn test_credentials() {
        let config = CyxWizApiSettings::default();
        let mut client = CyxWizApiClient::new(&config).unwrap();

        assert!(!client.is_registered());

        client.set_credentials("test-key".to_string(), "test-machine".to_string());

        assert!(client.is_registered());
        assert_eq!(client.api_key(), Some("test-key"));
        assert_eq!(client.machine_id(), Some("test-machine"));
    }
}
