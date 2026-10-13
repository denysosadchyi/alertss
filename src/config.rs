//! Persistent settings in %APPDATA%\SirenBar\config.json.

use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    /// Region name exactly as the data source spells it.
    pub region: String,
    pub sound: bool,
    pub animation: bool,
    /// Optional alerts.in.ua API token (enables the official source with alert levels).
    pub api_token: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { region: "Вінницька область".into(), sound: true, animation: true, api_token: String::new() }
    }
}

impl Config {
    /// Load from disk; on any error return `Config::default()`.
    pub fn load() -> Self {
        match config_path() {
            None => return Config::default(),
            Some(path) => {
                match fs::read_to_string(&path) {
                    Ok(contents) => match serde_json::from_str::<Config>(&contents) {
                        Ok(config) => return config,
                        Err(_) => return Config::default(),
                    },
                    Err(_) => return Config::default(),
                }
            }
        }
    }
    /// Save to disk (create the directory if needed). Errors are ignored.
    pub fn save(&self) {
        if let Some(path) = config_path() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            // The token is edited by hand in the file while we run: never overwrite it with a stale copy.
            let mut out = self.clone();
            out.api_token = Config::load().api_token;
            if let Ok(json) = serde_json::to_string_pretty(&out) {
                let _ = fs::write(&path, json);
            }
        }
    }
}

impl Config {
    /// Full path of the config file (%APPDATA%\SirenBar\config.json).
    pub fn path() -> PathBuf {
        config_path().unwrap_or_else(|| PathBuf::from("config.json"))
    }
}

fn config_path() -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(|appdata| {
        PathBuf::from(appdata).join("SirenBar").join("config.json")
    })
}

/// The app used to be called Alertss: carry its settings (region, token) over once.
pub fn migrate_old() {
    let (Some(new), Ok(appdata)) = (config_path(), std::env::var("APPDATA")) else { return };
    let old = PathBuf::from(appdata).join("alertss").join("config.json");
    if !new.exists() && old.exists() {
        if let Some(dir) = new.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::copy(&old, &new);
    }
}
