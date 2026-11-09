use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub id: u64,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created: DateTime<Local>,
    #[serde(default)]
    pub source: Option<String>,
}

pub struct Snippets {
    path: PathBuf,
    pub items: Vec<Snippet>,
}

impl Snippets {
    pub fn load(root: &Path) -> Self {
        let path = root.join("snippets.json");
        let items = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self { path, items }
    }
    pub fn save(&self) -> Result<()> {
        if let Some(p) = self.path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&self.path, serde_json::to_vec_pretty(&self.items)?)?;
        Ok(())
    }
    pub fn add(&mut self, title: &str, body: &str, tags: Vec<String>, source: Option<String>) -> Result<u64> {
        let id = self.items.iter().map(|s| s.id).max().unwrap_or(0) + 1;
        self.items.push(Snippet { id, title: title.into(), body: body.into(), tags, created: Local::now(), source });
        self.save()?;
        Ok(id)
    }
    pub fn remove(&mut self, id: u64) -> Result<()> {
        self.items.retain(|s| s.id != id);
        self.save()
    }
    pub fn search(&self, q: &str) -> Vec<&Snippet> {
        let q = q.to_lowercase();
        self.items
            .iter()
            .filter(|s| q.is_empty() || s.title.to_lowercase().contains(&q) || s.body.to_lowercase().contains(&q) || s.tags.iter().any(|t| t.to_lowercase().contains(&q)))
            .collect()
    }
}
