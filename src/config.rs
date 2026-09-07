use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Clone)]
pub struct Paths {
    pub client_secret: PathBuf,
    pub token_cache: PathBuf,
    pub database: PathBuf,
}

pub fn resolve() -> Result<Paths> {
    let config_dir = dirs::config_dir()
        .context("could not determine a config directory")?
        .join("lazycal");
    let data_dir = dirs::data_dir()
        .context("could not determine a data directory")?
        .join("lazycal");
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;

    Ok(Paths {
        client_secret: config_dir.join("client_secret.json"),
        token_cache: data_dir.join("tokencache.json"),
        database: data_dir.join("cache.db"),
    })
}
