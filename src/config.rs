use std::collections::HashMap;

use hocon::HoconLoader;
use indexmap::IndexMap;
use serde::Deserialize;

use simple_error::SimpleError;

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
    pub views: Option<HashMap<String, Vec<String>>>,
    #[serde(flatten)]
    pub ops: IndexMap<String, Operator>,
}

type DAG = IndexMap<String, Vec<String>>;

impl Config {
    pub fn from_file(path: &str) -> std::result::Result<Config, SimpleError> {
        HoconLoader::new()
            .load_file(path)
            .expect("")
            .resolve()
            .map_err(SimpleError::from)
    }

    pub fn build_dag(&self) -> std::result::Result<DAG, SimpleError> {
        // views
        if let Some(views) = &self.views {
            for (view_name, op_names) in (views).into_iter() {
                for op_name in op_names.into_iter() {
                    if !self.ops.contains_key(op_name) {
                        return Err(SimpleError::new(format!(
                            "{} in view {}",
                            op_name, view_name
                        )));
                    }
                }
            }
        }

        // dependencies
        for (op_name, ops) in (&self.ops).into_iter() {
            for dep_op_name in ops.depends_on.resolve().into_iter() {
                if op_name == &dep_op_name {
                    return Err(SimpleError::new(format!(
                        "dependency cannot be recursive in {}",
                        op_name
                    )));
                }

                if !self.ops.contains_key(&dep_op_name) {
                    return Err(SimpleError::new(format!(
                        "{} in op {}",
                        dep_op_name, op_name
                    )));
                }
            }
        }

        let mut order: Vec<String> = Vec::new();
        let mut poll = Vec::from_iter(self.ops.keys());

        while poll.len() > 0 {
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

            if satisfied.len() == 0 {
                return Err(SimpleError::new(format!(
                    "cycle detected with one of {}",
                    missing.into_iter().cloned().collect::<Vec<_>>().join(", ")
                )));
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
            .collect::<DAG>();
        Ok(dag)
    }
}
