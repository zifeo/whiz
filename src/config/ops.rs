use std::collections::HashSet;

use anyhow::{anyhow, bail, Result};
use indexmap::IndexMap;

use super::{Dag, Task};

pub type Ops = IndexMap<String, Task>;

pub fn build_dag(ops: &Ops) -> Result<Dag> {
    // dependencies
    for (op_name, task) in ops.iter() {
        for dep_op_name in task.depends_on.resolve().into_iter() {
            if op_name == &dep_op_name {
                return Err(anyhow!("dependency cannot be recursive in {}", op_name));
            }

            if !ops.contains_key(&dep_op_name) {
                return Err(anyhow!("{} in op {}", dep_op_name, op_name));
            }
        }
    }

    let mut order: Vec<String> = Vec::new();
    let mut poll = Vec::from_iter(ops.keys());

    while !poll.is_empty() {
        let (satisfied, missing): (Vec<&String>, Vec<&String>) =
            poll.into_iter().partition(|&item| {
                get_dependencies(ops, item)
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
            let nexts = ops
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

/// Returns the list of dependencies of a job defined in the config file.
pub fn get_dependencies(ops: &Ops, job_name: &str) -> Vec<String> {
    ops.get(job_name).unwrap().depends_on.resolve()
}

/// Returns a list of all the dependencies of a list of jobs, and
/// the children dependencies of each dependency recursively.
pub fn get_all_dependencies(ops: &Ops, jobs: &[String]) -> Vec<String> {
    let mut job_dependencies = Vec::new();
    let mut all_dependencies = Vec::new();

    // add initial dependencies
    for job_name in jobs {
        let child_dependencies = get_dependencies(ops, job_name);
        job_dependencies.extend(child_dependencies.into_iter());
    }

    // add child dependencies recursively
    while let Some(job_name) = job_dependencies.pop() {
        let child_dependencies = get_dependencies(ops, &job_name);
        job_dependencies.extend(child_dependencies.into_iter());
        all_dependencies.push(job_name);
    }

    all_dependencies
}

/// Returns the list of all the jobs defined in the config file.
pub fn get_jobs(ops: &Ops) -> Vec<&String> {
    ops.iter().map(|(job_name, _)| job_name).collect()
}

/// Returns the list of all the jobs set in the config file and
/// their dependencies in a simplified version.
pub fn get_formatted_list_of_jobs(ops: &Ops) -> String {
    let mut formatted_list_of_jobs: Vec<String> = get_jobs(ops)
        .iter()
        .map(|job_name| {
            let dependencies = get_dependencies(ops, job_name);
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

/// Filters the jobs to only the ones provided in `run`
/// and then recursively add their dependencies to be able
/// to run the filtered jobs.
///
/// Doesn't filter if `run` is empty.
///
/// Fails if a job in `run` is not set in the config file.
pub fn filter_jobs(ops: &mut Ops, run: &[String]) -> Result<()> {
    for job_name in run {
        if ops.get(job_name).is_none() {
            let formatted_list_of_jobs = get_formatted_list_of_jobs(ops);
            let error_header = format!("job '{job_name}' not found in config file.");
            let error_suggestion = format!("Valid jobs are:\n{formatted_list_of_jobs}");
            let error_message = format!("{error_header}\n\n{error_suggestion}");
            bail!(error_message);
        }
    }

    if !run.is_empty() {
        let mut filtered_jobs = get_all_dependencies(ops, run);
        filtered_jobs.extend(run.iter().cloned());
        let filtered_jobs: HashSet<String> = HashSet::from_iter(filtered_jobs);
        *ops = ops
            .clone()
            .into_iter()
            .filter(|(job_name, _)| filtered_jobs.contains(job_name))
            .collect();
    }

    Ok(())
}
