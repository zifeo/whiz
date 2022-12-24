use std::{path::{Path, PathBuf}, env};

pub fn find_config_path (location : &Path, config_name : &str) -> PathBuf {
    let config_name_as_path = Path::new(config_name);
    let mut config_path = location.to_path_buf();
    config_path.push(config_name_as_path);
    if config_path.exists() {
        return config_path;
    }

    let parent = location.parent();
    match parent {
        // not found
        None => config_name_as_path.to_path_buf(),
        // backtrack
        _ => find_config_path(parent.unwrap(), config_name)
    }
}

pub fn recurse_default_config (config_name : &str) -> String  {
    let cwd = env::current_dir().unwrap();
    // always returns a String without fail
    find_config_path(cwd.as_path().as_ref(), config_name)
            .into_os_string()
            .into_string()
            .unwrap()
}