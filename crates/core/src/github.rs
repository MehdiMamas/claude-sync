//! GitHub REST client, OAuth device flow, keychain token (§3, issue #13).
use serde::{Deserialize, Serialize};

pub const API: &str = "https://api.github.com";
pub const SCOPES: &str = "repo read:user";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Result of one poll of the device flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePoll {
    Pending,
    /// GitHub asked to slow down; new interval in seconds.
    SlowDown(u64),
    Token(String),
    Expired,
    Denied,
}

pub async fn start_device_flow(_client_id: &str) -> crate::Result<DeviceCode> {
    todo!()
}
pub async fn poll_device_flow(_client_id: &str, _device_code: &str) -> crate::Result<DevicePoll> {
    todo!()
}

/// OS keychain storage (service `claude-sync`). Never files, never logs.
pub mod token_store {
    pub fn load() -> crate::Result<Option<(String, String)>> {
        todo!("returns (login, token)")
    }
    pub fn save(_login: &str, _token: &str) -> crate::Result<()> {
        todo!()
    }
    pub fn delete() -> crate::Result<()> {
        todo!()
    }
}

/// Authenticated client. Cheap to clone.
#[derive(Clone)]
pub struct GitHub {
    pub token: String,
    pub api: String,
    pub http: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub id: u64,
    pub repo: crate::GroupId,
    pub inviter: String,
}

impl GitHub {
    pub fn new(token: String) -> Self {
        Self { token, api: API.into(), http: reqwest::Client::new() }
    }
    pub async fn user(&self) -> crate::Result<User> {
        todo!()
    }
}
