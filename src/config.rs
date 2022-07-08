use std::collections::HashMap;

use hocon::HoconLoader;
use serde::Deserialize;

use std::io;

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
    #[serde(flatten)]
    pub ops: HashMap<String, Operator>,
}

impl Config {
    pub fn from_file(path: &str) -> io::Result<Config> {
        let conf: Config = HoconLoader::new()
            .load_file(path)
            .expect("")
            .resolve()
            .unwrap();
        Ok(conf)
    }
}
