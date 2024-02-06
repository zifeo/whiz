use actix::System;
use anyhow::{anyhow, Result};
use crossterm::style::Stylize;
use std::{fs::File, path::PathBuf};

use crate::{args::Execute, config::Config, exec::ExecBuilder};

pub async fn start(opts: &Execute, config_file: File, base_dir: PathBuf) -> Result<()> {
    let config = Config::from_file(&config_file).map_err(|err| anyhow!("config error: {}", err))?;

    let mut queue: Vec<String> = Vec::new();
    queue.push(opts.task.clone());

    let mut executed_tasks: Vec<String> = Vec::new();

    while let Some(task_name) = queue.pop() {
        if !executed_tasks.is_empty() {
            println!();
        }

        let task = config
            .ops
            .get(&task_name)
            .ok_or_else(|| anyhow!("Task not found: {}", task_name))?;

        if executed_tasks.contains(&task_name) {
            continue;
        }

        let deps = task
            .depends_on
            .resolve()
            .into_iter()
            .filter(|dep| !executed_tasks.contains(dep))
            .collect::<Vec<_>>();
        if !deps.is_empty() {
            queue.push(task_name);
            queue.extend(deps);
            continue;
        }

        println!(
            "---------------- Starting task {task} ---------------",
            task = task_name.as_str().cyan(),
        );

        let exec_builder = ExecBuilder::new(task, &config, base_dir.clone()).await?;

        let exit_status = tokio::task::spawn_blocking(move || {
            let exec = exec_builder
                .build()
                .unwrap()
                .stdout(subprocess::Redirection::None)
                .stderr(subprocess::Redirection::None);
            let exit_status = exec.join().unwrap();
            return exit_status;
        })
        .await?;

        let prefix = if exit_status.success() {
            "✓".green()
        } else {
            "✖️".red()
        };

        println!(
            "---- {prefix} Task {task} exited with status {status} ----",
            task = task_name.as_str().cyan(),
            status = format!("{:?}", exit_status).yellow(),
        );

        System::current().stop_with_code(1);

        executed_tasks.push(task_name.clone());
    }

    Ok(())
}
