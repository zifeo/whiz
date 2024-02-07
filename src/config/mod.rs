use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::Deserialize;

use std::fs::File;
use std::io::Read;

pub mod color;
pub mod ops;
pub mod pipe;

use pipe::Pipe;

use self::{color::ColorOption, ops::Ops};

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
    pub command: Option<String>,
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

    #[serde(default)]
    pub color: IndexMap<String, String>,
}

#[derive(Deserialize, Debug)]
pub struct RawConfig {
    #[serde(default)]
    pub env: HashMap<String, String>,

    #[serde(flatten)]
    pub ops: IndexMap<String, Task>,
}

#[derive(Debug, Clone)]
pub struct ConfigInner {
    pub base_dir: Arc<Path>,
    pub env: HashMap<String, String>,
    pub ops: Ops,
    pub pipes_map: HashMap<String, Vec<Pipe>>,
    pub colors_map: HashMap<String, Vec<ColorOption>>,
}

impl ConfigInner {
    pub fn from_raw(config: RawConfig, base_dir: PathBuf) -> Result<Self> {
        let pipes_map = config
            .get_pipes_map()
            .context("Error while getting pipes")?;

        let colors_map = config
            .get_colors_map()
            .context("Error while getting colors")?;

        Ok(Self {
            base_dir: base_dir.into(),
            env: config.env,
            ops: config.ops,
            pipes_map,
            colors_map,
        })
    }
}

pub type Config = Arc<ConfigInner>;

pub type Dag = IndexMap<String, Vec<String>>;

impl FromStr for RawConfig {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_reader(s.as_bytes())
    }
}

impl RawConfig {
    pub fn from_file(file: &File) -> Result<RawConfig> {
        Self::from_reader(file)
    }

    fn from_reader(reader: impl Read) -> Result<RawConfig> {
        let mut config: serde_yaml::Value = serde_yaml::from_reader(reader)?;
        config.apply_merge()?;
        let mut config: RawConfig = serde_yaml::from_value(config)?;

        // make sure config file is a `Directed Acyclic Graph`
        ops::build_dag(&config.ops)?;

        config.simplify_dependencies();
        Ok(config)
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

    pub fn get_colors_map(&self) -> Result<HashMap<String, Vec<ColorOption>>> {
        let mut colors = HashMap::new();

        for (task_name, task) in &self.ops {
            let task_color_options: Vec<ColorOption> = task
                .color
                .iter()
                .filter_map(|color_config| ColorOption::from(color_config).ok())
                .collect();

            colors.insert(task_name.to_owned(), task_color_options);
        }

        Ok(colors)
    }

    /// Remove dependencies that are child of another dependency for
    /// the same job.
    pub fn simplify_dependencies(&mut self) {
        let jobs = self.ops.clone().into_iter().map(|(job_name, _)| job_name);
        for job_name in jobs {
            // array used to iterate all the elements and skip removed elements
            let mut dependencies = ops::get_dependencies(&self.ops, &job_name);
            let mut simplified_dependencies = dependencies.clone();

            while let Some(dependency) = dependencies.pop() {
                let child_dependencies =
                    &ops::get_all_dependencies(&self.ops, &[dependency.to_owned()]);
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

    fn filter_jobs(&mut self, run: &[String]) -> Result<()> {
        ops::filter_jobs(&mut self.ops, run)
    }
}

impl ConfigInner {
    pub fn build_dag(&self) -> Result<Dag> {
        ops::build_dag(&self.ops)
    }
}

pub struct ConfigBuilder {
    path: PathBuf,
    filter: Option<Vec<String>>,
}

impl ConfigBuilder {
    pub fn new(path: PathBuf) -> Self {
        Self { path, filter: None }
    }

    pub fn filter(mut self, filter: Vec<String>) -> Self {
        self.filter = Some(filter);
        self
    }

    pub fn build(self) -> Result<Config> {
        let file = File::open(&self.path)?;
        let mut config = RawConfig::from_file(&file)?;

        if let Some(filter) = self.filter {
            config
                .filter_jobs(&filter)
                .context("Error while filtering jobs")?;
        }

        Ok(Arc::new(ConfigInner::from_raw(
            config,
            self.path.parent().unwrap().into(),
        )?))
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
            let config: RawConfig = CONFIG_EXAMPLE.parse().unwrap();
            let jobs = &["c".to_string(), "z".to_string()];

            let jobs = ops::get_all_dependencies(&config.ops, jobs);
            let expected_jobs = vec!["a", "b", "y"];

            assert_array_not_strict!(jobs, expected_jobs);
        }

        #[test]
        fn gets_dependencies_from_config_file() {
            let config: RawConfig = CONFIG_EXAMPLE.parse().unwrap();

            let jobs = ops::get_dependencies(&config.ops, "c");
            let expected_jobs = vec!["b"];

            assert_array_not_strict!(jobs, expected_jobs);
        }

        #[test]
        fn simplifies_dependencies() {
            let config: RawConfig = CONFIG_EXAMPLE.parse().unwrap();

            let job_d = config.ops.get("d").unwrap();

            let dependencies_d = job_d.depends_on.resolve();
            let expected_dependencies = vec!["c", "z"];

            assert_array_not_strict!(dependencies_d, expected_dependencies);
        }

        #[test]
        fn resolves_alias() {
            let config: RawConfig = CONFIG_EXAMPLE.parse().unwrap();

            assert_array_not_strict!(
                ops::get_dependencies(&config.ops, "d"),
                ops::get_dependencies(&config.ops, "with_alias")
            );

            let job_with_alias = config.ops.get("with_alias").unwrap();
            assert_eq!(&job_with_alias.command.clone().unwrap(), "echo with_alias");
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
            let mut config: RawConfig = CONFIG_EXAMPLE.parse().unwrap();
            let run = &vec!["test".to_string()];

            config.filter_jobs(run).unwrap();

            let jobs: Vec<_> = config.ops.iter().map(|(job_name, _)| job_name).collect();
            let expected_jobs = vec!["test", "test_dependency"];

            assert_array_not_strict!(jobs, expected_jobs);
        }

        #[test]
        fn fails_job_filtering() {
            let mut config: RawConfig = CONFIG_EXAMPLE.parse().unwrap();

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
            let mut config: RawConfig = CONFIG_EXAMPLE.parse().unwrap();
            let run = &Vec::new();

            config.filter_jobs(run).unwrap();

            let jobs: Vec<_> = config.ops.iter().map(|(job_name, _)| job_name).collect();
            let expected_jobs = vec!["test", "test_dependency", "not_test_dependency"];

            assert_array_not_strict!(jobs, expected_jobs);
        }
    }

    mod colors {
        use regex::Regex;

        use super::*;

        const CONFIG_EXAMPLE: &str = r#"
            task1:
                color:
                    "^abc": red
                    "My": yellow
            task2:
                color:
                    "d+": '#def'
            "#;

        #[test]
        fn parse_colors_map() {
            let config: RawConfig = CONFIG_EXAMPLE.parse().unwrap();
            let actual = config.get_colors_map().unwrap();
            let mut expected = HashMap::new();

            expected.insert(
                "task1".to_owned(),
                vec![
                    ColorOption::new(
                        Regex::from_str("^abc").unwrap(),
                        ColorOption::parse_color("red").unwrap(),
                    ),
                    ColorOption::new(
                        Regex::from_str("My").unwrap(),
                        ColorOption::parse_color("yellow").unwrap(),
                    ),
                ],
            );
            expected.insert(
                "task2".to_owned(),
                vec![ColorOption::new(
                    Regex::from_str("d+").unwrap(),
                    ColorOption::parse_color("#def").unwrap(),
                )],
            );

            assert_eq!(actual.get("task1").unwrap(), expected.get("task1").unwrap());
            assert_eq!(actual.get("task2").unwrap(), expected.get("task2").unwrap());
        }
    }
}
