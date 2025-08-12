use anyhow::Result;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Config {
    pub servers: Vec<Server>,
}

#[derive(Debug, Clone)]
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
        let mut servers = Vec::new();

        // inject default server from environment variable if available
        if let Some(default_server) = option_env!("REMDIT_DEFAULT_SERVER") {
            if !default_server.is_empty() {
                servers.push(Server {
                    addr: default_server.to_string(),
                    key: None,
                });
            }
        }

        Self { servers }
    }
}

pub async fn load_config() -> Result<Config> {
    let home_dir = home::home_dir().unwrap_or_else(|| "/tmp".into());
    let home_config: String = format!("{}/.remdit/config.toml", home_dir.display());

    let config_paths = vec!["/etc/remdit/config.toml", &home_config, "config.toml"];

    for path in config_paths {
        if Path::new(path).exists() {
            let content = tokio::fs::read_to_string(path).await?;
            let config = parse_config(&content)?;
            eprintln!("Loaded config from: {}", path);
            return Ok(config);
        }
    }

    eprintln!("No config file found, using default config");
    Ok(Config::default())
}

// Simple TOML parser for our specific config format
fn parse_config(content: &str) -> Result<Config> {
    let mut servers = Vec::new();
    let mut current_server: Option<Server> = None;
    let mut in_servers_array = false;

    for line in content.lines() {
        let line = line.trim();
        
        // Skip comments and empty lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Check for [[servers]] section
        if line == "[[servers]]" {
            // Save previous server if exists
            if let Some(server) = current_server.take() {
                servers.push(server);
            }
            current_server = Some(Server {
                addr: String::new(),
                key: None,
            });
            in_servers_array = true;
            continue;
        }

        if in_servers_array {
            if let Some(ref mut server) = current_server {
                if let Some((key, value)) = parse_key_value(line) {
                    match key {
                        "addr" => server.addr = value,
                        "key" => server.key = Some(value),
                        _ => {} // Ignore unknown keys
                    }
                }
            }
        }
    }

    // Save the last server
    if let Some(server) = current_server {
        servers.push(server);
    }

    Ok(Config { servers })
}

// Parse a line like 'key = "value"' or 'key = value'
fn parse_key_value(line: &str) -> Option<(&str, String)> {
    let mut parts = line.splitn(2, '=');
    let key = parts.next()?.trim();
    let value = parts.next()?.trim();
    
    // Remove quotes if present
    let value = if (value.starts_with('"') && value.ends_with('"')) ||
                  (value.starts_with('\'') && value.ends_with('\'')) {
        &value[1..value.len()-1]
    } else {
        value
    };
    
    Some((key, value.to_string()))
}
