use std::{
    env,
    fs::File,
    io::{Error, ErrorKind},
    path::{Path, PathBuf},
};

pub fn find_config_path(location: &Path, config_name: &str) -> Result<PathBuf, std::io::Error> {
    let config_name_as_path = Path::new(config_name);
    let mut config_path = location.to_path_buf();
    config_path.push(config_name_as_path);
    if config_path.exists() {
        return Ok(config_path);
    }

    let parent = location.parent();
    match parent {
        // not found
        None => {
            let message = format!("configuration file {} not found", config_name);
            Err(Error::new(ErrorKind::NotFound, message))
        }
        // backtrack
        Some(parent) => find_config_path(parent, config_name),
    }
}

pub fn recurse_config_file(config_name: &str) -> Result<(File, PathBuf), anyhow::Error> {
    let cwd = env::current_dir().unwrap();
    let path = find_config_path(cwd.as_path(), config_name)?;
    let config_file = File::open(&path).unwrap();

    Ok((config_file, path))
}

#[inline]
pub fn default_to_true() -> bool {
    true
}