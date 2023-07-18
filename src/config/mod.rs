use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
};

use anyhow::{anyhow, bail, Result};
use indexmap::IndexMap;
use serde::Deserialize;

use std::fs::File;
use std::io::Read;

pub mod pipe;

use pipe::Pipe;

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(untagged)]
pub enum Lift<T> {
    More(Vec<T>),
    One(T),
    #[default]
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

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub workdir: Option<String>,
    pub command: String,
    pub entrypoint: Option<String>,

    #[serde(default)]
    pub watch: Lift<String>,

    #[serde(default)]
    pub ignore: Lift<String>,

    #[serde(default)]
    pub env: HashMap<String, String>,

    #[serde(default)]
    pub env_file: Lift<String>,

    #[serde(default)]
    pub depends_on: Lift<String>,

    /// Map of output redirections with the format:
    /// `regular expressiong` -> `pipe`
    ///
    /// Where the content matched by the regular expression
    /// can be redirected to:
    ///
    /// - whiz: creating a new tab for the incoming messages.
    /// Format: `whiz://{tab_name}`
    ///
    /// - /dev/null: silence the matched content.
    /// Format: `/dev/null` or `file:///dev/null`
    ///
    /// - file: saving the matched content in a log file.
    /// Format: `path` or `file:///{path}`
    ///
    /// # NOTE
    ///
    /// Any other output not matched by a regular expression goes to
    /// `whiz://{task_name}` as default.
    #[serde(default)]
    pub pipe: HashMap<String, String>,
}

#[derive(Deserialize, Debug)]
pub struct Config {
    #[serde(default)]
    pub env: HashMap<String, String>,

    #[serde(flatten)]
    pub ops: IndexMap<String, Task>,
}

pub type Dag = IndexMap<String, Vec<String>>;

impl FromStr for Config {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_reader(s.as_bytes())
    }
}

impl Config {
    pub fn from_file(file: &File) -> Result<Config> {
        Self::from_reader(file)
    }

    fn from_reader(reader: impl Read) -> Result<Config> {
        let mut config: serde_yaml::Value = serde_yaml::from_reader(reader)?;
        config.apply_merge()?;
        let mut config: Config = serde_yaml::from_value(config)?;

        // make sure config file is a `Directed Acyclic Graph`
        config.build_dag()?;

        config.simplify_dependencies();
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
                let formatted_list_of_jobs = self.get_formatted_list_of_jobs();
                let error_header = format!("job '{job_name}' not found in config file.");
                let error_suggestion = format!("Valid jobs are:\n{formatted_list_of_jobs}");
                let error_message = format!("{error_header}\n\n{error_suggestion}");
                bail!(error_message);
            }
        }

        if !run.is_empty() {
            let mut filtered_jobs = self.get_all_dependencies(run);
            filtered_jobs.extend(run.clone().into_iter());
            let filtered_jobs: HashSet<String> = HashSet::from_iter(filtered_jobs.into_iter());
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
                    self.get_dependencies(item)
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

    /// Returns a list of all the dependencies of a list of jobs, and
    /// the children dependencies of each dependency recursively.
    pub fn get_all_dependencies(&self, jobs: &[String]) -> Vec<String> {
        let mut job_dependencies = Vec::new();
        let mut all_dependencies = Vec::new();

        // add initial dependencies
        for job_name in jobs {
            let child_dependencies = self.get_dependencies(job_name);
            job_dependencies.extend(child_dependencies.into_iter());
        }

        // add child dependencies recursively
        while let Some(job_name) = job_dependencies.pop() {
            let child_dependencies = self.get_dependencies(&job_name);
            job_dependencies.extend(child_dependencies.into_iter());
            all_dependencies.push(job_name);
        }

        all_dependencies
    }

    /// Returns the list of dependencies of a job defined in the config file.
    pub fn get_dependencies(&self, job_name: &str) -> Vec<String> {
        self.ops.get(job_name).unwrap().depends_on.resolve()
    }

    /// Returns the list of all the jobs set in the config file and
    /// their dependencies in a simplified version.
    pub fn get_formatted_list_of_jobs(&self) -> String {
        let mut formatted_list_of_jobs: Vec<String> = self
            .get_jobs()
            .iter()
            .map(|job_name| {
                let dependencies = self.get_dependencies(job_name);
                let mut formatted_job = format!("  - {job_name}");

                if !dependencies.is_empty() {
                    formatted_job += &format!(" ({})", dependencies.join(","));
                }

                formatted_job
            })
            .collect();
        formatted_list_of_jobs.sort();
        formatted_list_of_jobs.join("\n")
    }

    /// Returns the list of all the jobs defined in the config file.
    pub fn get_jobs(&self) -> Vec<&String> {
        self.ops.iter().map(|(job_name, _)| job_name).collect()
    }

    /// Parses the pipes of each task to make sure they are valid and returns
    /// a [`HashMap`] where the keys are the task names and the values
    /// are the parsed pipes.
    pub fn get_pipes_map(&self) -> Result<HashMap<String, Vec<Pipe>>> {
        let mut pipes = HashMap::new();

        for (task_name, task) in &self.ops {
            for pipe_config in &task.pipe {
                let task_pipes: &mut Vec<Pipe> = pipes.entry(task_name.to_owned()).or_default();
                let pipe = Pipe::from(pipe_config)?;
                task_pipes.push(pipe);
            }
        }

        Ok(pipes)
    }

    /// Remove dependencies that are child of another dependency for
    /// the same job.
    pub fn simplify_dependencies(&mut self) {
        let jobs = self.ops.clone().into_iter().map(|(job_name, _)| job_name);
        for job_name in jobs {
            // array used to iterate all the elements and skip removed elements
            let mut dependencies = self.get_dependencies(&job_name);
            let mut simplified_dependencies = dependencies.clone();

            while let Some(dependency) = dependencies.pop() {
                let child_dependencies = &self.get_all_dependencies(&[dependency.to_owned()]);
                let child_dependencies: HashSet<&String> =
                    HashSet::from_iter(child_dependencies.iter());
                // remove all the dependencies that are dependency
                // of the current `dependency`
                dependencies.retain(|job_name| !child_dependencies.contains(job_name));
                simplified_dependencies.retain(|job_name| !child_dependencies.contains(job_name));
            }

            let job_operator = self.ops.get_mut(&job_name).unwrap();
            job_operator.depends_on = Lift::More(simplified_dependencies);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts if two arrays are equal without taking into account the order.
    macro_rules! assert_array_not_strict {
        ($left:expr, $right:expr) => {
            match (&$left, &$right) {
                (left_val, right_val) => {
                    let mut v1 = left_val.clone();
                    v1.sort();
                    let mut v2 = right_val.clone();
                    v2.sort();
                    assert_eq!(v1, v2);
                }
            };
        };
    }

    mod dependencies {
        use super::*;

        const CONFIG_EXAMPLE: &str = r#"
            a:
                command: echo a

            b:
                command: echo b
                depends_on: 
                    - a

            c:
                command: echo c
                depends_on:
                    - b

            d: &alias
                command: echo c
                depends_on:
                    - a
                    - b
                    - c
                    - y
                    - z

            y:
                command: echo y

            z:
                command: echo z
                depends_on:
                    - y

            not_child_dependency:
                command: echo hello world

            with_alias:
                <<: *alias
                command: echo with_alias
        "#;

        #[test]
        fn gets_all_dependencies() {
            let config: Config = CONFIG_EXAMPLE.parse().unwrap();
            let jobs = &["c".to_string(), "z".to_string()];

            let jobs = config.get_all_dependencies(jobs);
            let expected_jobs = vec!["a", "b", "y"];

            assert_array_not_strict!(jobs, expected_jobs);
        }

        #[test]
        fn gets_dependencies_from_config_file() {
            let config: Config = CONFIG_EXAMPLE.parse().unwrap();

            let jobs = config.get_dependencies("c");
            let expected_jobs = vec!["b"];

            assert_array_not_strict!(jobs, expected_jobs);
        }

        #[test]
        fn simplifies_dependencies() {
            let config: Config = CONFIG_EXAMPLE.parse().unwrap();

            let job_d = config.ops.get("d").unwrap();

            let dependencies_d = job_d.depends_on.resolve();
            let expected_dependencies = vec!["c", "z"];

            assert_array_not_strict!(dependencies_d, expected_dependencies);
        }

        #[test]
        fn resolves_alias() {
            let config: Config = CONFIG_EXAMPLE.parse().unwrap();

            assert_array_not_strict!(
                config.get_dependencies("d"),
                config.get_dependencies("with_alias")
            );

            let job_with_alias = config.ops.get("with_alias").unwrap();
            assert_eq!(&job_with_alias.command, "echo with_alias");
        }
    }

    mod job_filtering {
        use super::*;

        const CONFIG_EXAMPLE: &str = r#"
            not_test_dependency:
                command: echo fails

            test_dependency:
                command: echo hello

            test:
                command: echo world
                depends_on:
                    - test_dependency
        "#;

        #[test]
        fn filters_jobs() {
            let mut config: Config = CONFIG_EXAMPLE.parse().unwrap();
            let run = &vec!["test".to_string()];

            config.filter_jobs(run).unwrap();

            let jobs: Vec<_> = config.ops.iter().map(|(job_name, _)| job_name).collect();
            let expected_jobs = vec!["test", "test_dependency"];

            assert_array_not_strict!(jobs, expected_jobs);
        }

        #[test]
        fn fails_job_filtering() {
            let mut config: Config = CONFIG_EXAMPLE.parse().unwrap();

            let expected_err = vec![
                "job 'doesnt_exist' not found in config file.",
                "",
                "Valid jobs are:",
                "  - not_test_dependency",
                "  - test (test_dependency)",
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

            let jobs: Vec<_> = config.ops.iter().map(|(job_name, _)| job_name).collect();
            let expected_jobs = vec!["test", "test_dependency", "not_test_dependency"];

            assert_array_not_strict!(jobs, expected_jobs);
        }
    }
}
