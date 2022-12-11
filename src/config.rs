use std::{
    collections::{HashMap, HashSet},
    io,
    str::FromStr,
};

use anyhow::{anyhow, bail, Result};
use indexmap::IndexMap;
use serde::Deserialize;

use std::fs::File;

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Lift<T> {
    More(Vec<T>),
    One(T),
    Empty,
}

impl<T: std::clone::Clone> Lift<T> {
    pub fn resolve(&self) -> Vec<T> {
        match self {
            Lift::More(vs) => vs.clone(),
            Lift::One(v) => vec![v.clone()],
            Lift::Empty => vec![],
        }
    }
}

impl<T> Default for Lift<T> {
    fn default() -> Self {
        Lift::Empty
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct Operator {
    pub workdir: Option<String>,
    pub shell: String,

    #[serde(default)]
    pub watches: Lift<String>,

    #[serde(default)]
    pub ignores: Lift<String>,

    #[serde(default)]
    pub envs: Option<HashMap<String, String>>,

    #[serde(default)]
    pub depends_on: Lift<String>,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    #[serde(default)]
    pub views: HashMap<String, Vec<String>>,

    #[serde(default)]
    pub envs: HashMap<String, String>,

    #[serde(flatten)]
    pub ops: IndexMap<String, Operator>,
}

pub type Dag = IndexMap<String, Vec<String>>;

impl FromStr for Config {
    type Err = serde_yaml::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let config: Config = serde_yaml::from_str(s)?;
        Ok(config)
    }
}

impl Config {
    pub fn from_file(path: &str) -> Result<Config> {
        let file = File::open(path).map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => anyhow!("file {} not found", path),
            _ => anyhow!(err.to_string()),
        })?;
        let config: Config = serde_yaml::from_reader(file)?;
        Ok(config)
    }

    /// Filters the jobs to only the ones provided in `run`
    /// and then recursively add their dependencies to be able
    /// to run the filtered jobs.
    ///
    /// Doesn't filter if `run` is empty.
    ///
    /// Fails if a job in `run` is not set in the config file.
    pub fn filter_jobs(&mut self, run: &Vec<String>) -> Result<()> {
        for job_name in run {
            if self.ops.get(job_name).is_none() {
                let mut formatted_list_of_jobs = self
                    .ops
                    .iter()
                    .map(|(job_name, _)| format!("  - {job_name}"))
                    .collect::<Vec<String>>();
                formatted_list_of_jobs.sort();
                let formatted_list_of_jobs = formatted_list_of_jobs.join("\n");
                let error_header = format!("job '{job_name}' not found in config file.");
                let error_sugesstion = format!("Valid jobs are:\n{formatted_list_of_jobs}");
                let error_message = format!("{error_header}\n\n{error_sugesstion}");
                bail!(error_message);
            }
        }

        if !run.is_empty() {
            let mut filtered_jobs: HashSet<String> = HashSet::new();
            let mut job_dependencies: Vec<String> = run.clone();

            while let Some(job_name) = job_dependencies.pop() {
                let child_dependencies = self
                    .ops
                    .get(&job_name)
                    .unwrap()
                    .depends_on
                    .resolve()
                    .into_iter();
                job_dependencies.extend(child_dependencies);
                filtered_jobs.insert(job_name);
            }

            self.ops = self
                .ops
                .clone()
                .into_iter()
                .filter(|(job_name, _)| filtered_jobs.contains(job_name))
                .collect();
        }

        Ok(())
    }

    pub fn build_dag(&self) -> Result<Dag> {
        // views
        for (view_name, op_names) in self.views.iter() {
            for op_name in op_names.iter() {
                if !self.ops.contains_key(op_name) {
                    return Err(anyhow!("{} in view {}", op_name, view_name));
                }
            }
        }

        // dependencies
        for (op_name, ops) in (&self.ops).into_iter() {
            for dep_op_name in ops.depends_on.resolve().into_iter() {
                if op_name == &dep_op_name {
                    return Err(anyhow!("dependency cannot be recursive in {}", op_name));
                }

                if !self.ops.contains_key(&dep_op_name) {
                    return Err(anyhow!("{} in op {}", dep_op_name, op_name));
                }
            }
        }

        let mut order: Vec<String> = Vec::new();
        let mut poll = Vec::from_iter(self.ops.keys());

        while !poll.is_empty() {
            let (satisfied, missing): (Vec<&String>, Vec<&String>) =
                poll.into_iter().partition(|&item| {
                    self.ops
                        .get(item)
                        .unwrap()
                        .depends_on
                        .resolve()
                        .iter()
                        .all(|p| order.contains(p))
                });

            if satisfied.is_empty() {
                return Err(anyhow!(
                    "cycle detected with one of {}",
                    missing.into_iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }

            order.extend(satisfied.into_iter().cloned().collect::<Vec<_>>());
            poll = missing;
        }

        let dag = order
            .into_iter()
            .map(|item| {
                let nexts = self
                    .ops
                    .iter()
                    .filter(|(_, op)| op.depends_on.resolve().contains(&item))
                    .map(|(op_name, _)| op_name.clone())
                    .collect::<Vec<_>>();
                (item, nexts)
            })
            .rev()
            .collect::<Dag>();
        Ok(dag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod job_filtering {
        use super::*;

        const CONFIG_EXAMPLE: &str = r#"
            not_test_dependency:
                shell: echo fails

            test_dependency:
                shell: echo hello

            test:
                shell: echo world
                depends_on:
                    - test_dependency
        "#;

        #[test]
        fn filters_jobs() {
            let mut config: Config = CONFIG_EXAMPLE.parse().unwrap();
            let run = &vec!["test".to_string()];

            config.filter_jobs(run).unwrap();

            let mut jobs: Vec<_> = config.ops.iter().map(|(job_name, _)| job_name).collect();
            let mut expected_jobs = vec!["test", "test_dependency"];

            // sorting arrays because the order of the jobs after filtering does not matter
            assert_eq!(jobs.sort(), expected_jobs.sort());
        }

        #[test]
        fn fails_job_filtering() {
            let mut config: Config = CONFIG_EXAMPLE.parse().unwrap();

            let expected_err = vec![
                "job 'doesnt_exist' not found in config file.",
                "",
                "Valid jobs are:",
                "  - not_test_dependency",
                "  - test",
                "  - test_dependency",
            ]
            .join("\n");

            let mut err_message = String::new();
            let run = &vec!["doesnt_exist".to_string()];

            if let Err(err) = config.filter_jobs(run) {
                err_message = err.to_string();
            };

            assert_eq!(err_message, expected_err);
        }

        #[test]
        fn doesnt_filter_jobs() {
            let mut config: Config = CONFIG_EXAMPLE.parse().unwrap();
            let run = &Vec::new();

            config.filter_jobs(run).unwrap();

            let mut jobs: Vec<_> = config.ops.iter().map(|(job_name, _)| job_name).collect();
            let mut expected_jobs = vec!["test", "test_dependency", "not_test_dependency"];

            // sorting arrays because the order of the jobs after filtering does not matter
            assert_eq!(jobs.sort(), expected_jobs.sort());
        }
    }
}
