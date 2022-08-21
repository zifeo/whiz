use std::{collections::HashMap, env, io, str::FromStr};

use anyhow::{anyhow, Result};
use indexmap::IndexMap;
use regex::Regex;
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

impl Operator {
    pub fn resolve_envs(&self) -> Result<HashMap<String, String>> {
        let re = Regex::new(r"(\$\{?(\w+)\}?)")?;
        let vars: HashMap<String, String> = HashMap::from_iter(env::vars());
        let res = match &self.envs {
            None => HashMap::default(),
            Some(kvs) => kvs
                .iter()
                .map(|(key, value)| {
                    let hydration = re.captures_iter(value).fold(value.clone(), |agg, c| {
                        agg.replace(&c[1], vars.get(&c[2]).unwrap_or(&"".to_string()))
                    });
                    (key.clone(), hydration)
                })
                .collect::<HashMap<_, _>>(),
        };
        Ok(res)
    }
}

#[derive(Deserialize, Debug)]
pub struct Config {
    pub views: Option<HashMap<String, Vec<String>>>,
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

    pub fn build_dag(&self) -> Result<Dag> {
        // views
        if let Some(views) = &self.views {
            for (view_name, op_names) in (views).iter() {
                for op_name in op_names.iter() {
                    if !self.ops.contains_key(op_name) {
                        return Err(anyhow!("{} in view {}", op_name, view_name));
                    }
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
