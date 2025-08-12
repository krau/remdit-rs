use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub servers: Vec<Server>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Server {
    pub addr: String,
    pub key: Option<String>,
}

impl Server {
    pub fn is_valid(&self) -> bool {
        !self.addr.is_empty()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
        }
    }
}

pub async fn load_config() -> Result<Config> {
    let home_dir = home::home_dir().unwrap_or_else(|| "/tmp".into());
    let home_config: String = format!("{}/.remdit/config.toml", home_dir.display());

    let config_paths = vec!["/etc/remdit/config.toml", &home_config, "config.toml"];

    for path in config_paths {
        if Path::new(path).exists() {
            let content = tokio::fs::read_to_string(path).await?;
            let config: Config = basic_toml::from_str(&content)?;
            eprintln!("Loaded config from: {}", path);
            return Ok(config);
        }
    }

    eprintln!("No config file found, using default config");
    Ok(Config::default())
}
