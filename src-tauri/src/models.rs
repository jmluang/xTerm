use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Host {
    pub id: String,
    #[serde(rename = "sortOrder")]
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub alias: String,
    pub hostname: String,
    #[serde(default)]
    pub user: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(rename = "hasPassword")]
    #[serde(default)]
    pub has_password: bool,
    #[serde(rename = "identityFile")]
    pub identity_file: Option<String>,
    #[serde(rename = "proxyJump")]
    pub proxy_jump: Option<String>,
    #[serde(rename = "envVars")]
    #[serde(default)]
    pub env_vars: Option<String>,
    #[serde(default)]
    pub encoding: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(rename = "updatedAt")]
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub deleted: bool,
}

fn default_port() -> u16 {
    22
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Settings {
    pub webdav_url: Option<String>,
    #[serde(default)]
    pub webdav_folder: Option<String>,
    pub webdav_username: Option<String>,
    pub webdav_password: Option<String>,
}
