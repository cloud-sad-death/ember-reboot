use std::path::PathBuf;

/// `~/Library/Application Support/Ember` on macOS (XDG data dir elsewhere); EMBER_DATA overrides.
pub fn data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("EMBER_DATA") {
        return PathBuf::from(p);
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("Ember")
}

pub fn config_file() -> PathBuf {
    if let Ok(p) = std::env::var("EMBER_CONFIG") {
        return PathBuf::from(p);
    }
    data_dir().join("ember.toml")
}
