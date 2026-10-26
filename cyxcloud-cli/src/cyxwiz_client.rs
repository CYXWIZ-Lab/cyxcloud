//! CyxWiz API Client
//!
//! HTTP client for authenticating with the CyxWiz API service.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

/// CyxWiz API client errors
#[derive(Error, Debug)]
pub enum CyxWizError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Account not found")]
    AccountNotFound,

    #[error("Email already registered")]
    EmailAlreadyExists,

    #[error("API error: {status} - {message}")]
    Api { status: u16, message: String },
}

pub type Result<T> = std::result::Result<T, CyxWizError>;

/// Login request payload
#[derive(Debug, Serialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Register request payload (the API's POST /api/users)
#[derive(Debug, Serialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Authentication response from login/register
#[derive(Debug, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: AuthUser,
}

/// User info in auth response
#[derive(Debug, Deserialize, Clone)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    pub username: Option<String>,
}

/// CyxWallet info
#[derive(Debug, Deserialize, Clone)]
pub struct CyxWallet {
    #[serde(rename = "publicKey")]
    pub public_key: String,
}

/// User profile response (from the API's /api/users/me)
#[derive(Debug, Deserialize)]
pub struct UserProfile {
    pub email: String,
    pub username: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "cyxWallet")]
    pub cyx_wallet: Option<CyxWallet>,
    #[serde(rename = "externalWallet")]
    pub external_wallet: Option<String>,
}

/// CyxWiz API client
pub struct CyxWizClient {
    client: Client,
    base_url: String,
}

impl CyxWizClient {
    /// Create a new CyxWiz API client
    pub fn new(base_url: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Login with email and password
    pub async fn login(&self, email: &str, password: &str) -> Result<AuthResponse> {
        let url = format!("{}/api/auth/login", self.base_url);

        let request = LoginRequest {
            email: email.to_string(),
            password: password.to_string(),
        };

        let response = self.client.post(&url).json(&request).send().await?;

        let status = response.status();

        if status.is_success() {
            let auth: AuthResponse = response.json().await?;
            Ok(auth)
        } else if status.as_u16() == 401 {
            Err(CyxWizError::InvalidCredentials)
        } else if status.as_u16() == 404 {
            Err(CyxWizError::AccountNotFound)
        } else {
            let message = response.text().await.unwrap_or_default();
            Err(CyxWizError::Api {
                status: status.as_u16(),
                message,
            })
        }
    }

    /// Register a new account (the API's POST /api/users), then sign in:
    /// creating a user returns the user, not a token.
    pub async fn register(
        &self,
        email: &str,
        password: &str,
        username: Option<&str>,
    ) -> Result<AuthResponse> {
        let url = format!("{}/api/users", self.base_url);

        let request = RegisterRequest {
            email: email.to_string(),
            password: password.to_string(),
            username: username
                .map(|s| s.to_string())
                .unwrap_or_else(|| email.split('@').next().unwrap_or(email).to_string()),
            name: None,
        };

        let response = self.client.post(&url).json(&request).send().await?;

        let status = response.status();

        if status.is_success() {
            self.login(email, password).await
        } else if status.as_u16() == 409 {
            Err(CyxWizError::EmailAlreadyExists)
        } else {
            let message = response.text().await.unwrap_or_default();
            Err(CyxWizError::Api {
                status: status.as_u16(),
                message,
            })
        }
    }

    /// Get current user profile
    pub async fn get_profile(&self, token: &str) -> Result<UserProfile> {
        let url = format!("{}/api/users/me", self.base_url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        let status = response.status();

        if status.is_success() {
            let profile: UserProfile = response.json().await?;
            Ok(profile)
        } else if status.as_u16() == 401 {
            Err(CyxWizError::InvalidCredentials)
        } else {
            let message = response.text().await.unwrap_or_default();
            Err(CyxWizError::Api {
                status: status.as_u16(),
                message,
            })
        }
    }
}
