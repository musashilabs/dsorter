mod pid;
mod status;
pub use status::*;

use crate::error::ConfigError;
use directories::ProjectDirs;
pub use pid::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;

use std::path::{Path, PathBuf};

#[derive(Deserialize, Debug)]
pub struct Config {
    pub watch: WatchConfig,
    pub destinations: HashMap<String, String>,
    pub extensions: HashMap<String, Vec<String>>,
    pub partial: PartialConfig,
}

#[derive(Deserialize, Debug)]
pub struct PartialConfig {
    pub extensions: Vec<String>,
}

#[derive(Deserialize, Debug)]
pub struct WatchConfig {
    pub path: String,
}

const DEFAULT_CONFIG: &str = include_str!("../../default_config.toml");

pub fn config_path() -> Result<PathBuf, ConfigError> {
    let proj_dirs = ProjectDirs::from("com", "rohit", "dsorter").ok_or(ConfigError::NoConfigDir)?;
    Ok(proj_dirs.config_dir().join("config.toml"))
}

pub fn load_or_create_config(override_path: Option<&Path>) -> Result<Config, ConfigError> {
    let path = match override_path {
        Some(p) => p.to_path_buf(),
        None => config_path()?,
    };

    if !path.exists() {
        let parent = path.parent().ok_or(ConfigError::NoConfigDir)?;
        fs::create_dir_all(parent)?;
        fs::write(&path, DEFAULT_CONFIG)?;
    }

    let contents = fs::read_to_string(&path)?;
    let config = toml::from_str(&contents)?;
    Ok(config)
}

pub fn log_path() -> Result<PathBuf, ConfigError> {
    let proj_dirs = ProjectDirs::from("com", "rohit", "dsorter").ok_or(ConfigError::NoConfigDir)?;
    let data_dir = proj_dirs.data_dir();
    fs::create_dir_all(data_dir)?;
    Ok(data_dir.join("dsorter.log"))
}
