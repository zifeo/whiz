use anyhow::{Context, Result};
use dotenv_parser::parse_dotenv;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
use subprocess::Exec;

use crate::config::{Config, ConfigInner, Task};

impl Task {
    pub fn get_absolute_workdir(&self, base_dir: &Path) -> PathBuf {
        match &self.workdir {
            Some(path) => base_dir.join(path),
            None => base_dir.to_path_buf(),
        }
    }
}

pub struct ExecBuilder {
    env: Vec<(String, String)>,
    cwd: PathBuf,
    cmd: String,
    args: Vec<String>,
}

impl ExecBuilder {
    pub async fn new(task: &Task, config: &Config) -> Result<Self> {
        let cwd = task.get_absolute_workdir(&config.base_dir);

        let shared_env = config.get_shared_env().await?;
        let env = task
            .get_full_env(&cwd, &shared_env)
            .await?
            .into_iter()
            .collect::<Vec<_>>();

        let (cmd, args) = task.get_exec_command()?;

        Ok(Self {
            cwd,
            env,
            cmd,
            args,
        })
    }

    pub fn build(&self) -> Result<Exec> {
        Ok(Exec::cmd(self.cmd.clone())
            .args(&self.args)
            .cwd(&self.cwd)
            .env_extend(&self.env))
    }

    pub fn to_string(&self) -> String {
        format!("EXEC: {} {:?} at {:?}", self.cmd, self.args, self.cwd)
    }
}

impl ConfigInner {
    pub async fn get_shared_env(&self) -> Result<HashMap<String, String>> {
        let mut shared_env = HashMap::from_iter(std::env::vars());
        shared_env.extend(lade_sdk::resolve(&self.env, &shared_env)?);
        return lade_sdk::hydrate(shared_env, self.base_dir.to_path_buf()).await;
    }
}

impl Task {
    pub fn get_exec_command(&self) -> Result<(String, Vec<String>)> {
        let default_entrypoint = {
            #[cfg(not(target_os = "windows"))]
            {
                "bash -c"
            }

            #[cfg(target_os = "windows")]
            {
                "cmd /c"
            }
        };

        let entrypoint_lex = match &self.entrypoint {
            Some(e) => {
                if !e.is_empty() {
                    e.as_str()
                } else {
                    default_entrypoint
                }
            }
            None => default_entrypoint,
        };

        let entrypoint_split = {
            let mut s = shlex::split(entrypoint_lex).unwrap();

            match &self.command {
                Some(a) => {
                    s.push(a.to_owned());
                    s
                }
                None => s,
            }
        };

        let entrypoint = &entrypoint_split[0];
        let nargs = entrypoint_split[1..]
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<String>>();

        Ok((entrypoint.to_owned(), nargs))
    }

    pub async fn get_full_env(
        &self,
        cwd: &Path,
        shared_env: &HashMap<String, String>,
    ) -> Result<HashMap<String, String>> {
        let mut env = HashMap::default();

        for env_file in self.env_file.resolve() {
            let path = cwd.join(env_file.clone());
            let file = fs::read_to_string(path.clone())
                .with_context(|| format!("cannot find env_file {:?}", path.clone()))?;
            let values = parse_dotenv(&file)
                .map_err(anyhow::Error::msg)
                .with_context(|| format!("cannot parse env_file {:?}", path))?
                .into_iter()
                .map(|(k, v)| (k, v.replace("\\n", "\n")));

            env.extend(lade_sdk::resolve(&values.collect(), shared_env)?);
        }

        env.extend(lade_sdk::resolve(&self.env.clone(), shared_env)?);
        let mut env = lade_sdk::hydrate(env, cwd.to_owned()).await?;
        env.extend(shared_env.clone());

        Ok(env)
    }
}

pub fn get_env() -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("RUST_LOG".to_string(), "info".to_string());
    env
}
