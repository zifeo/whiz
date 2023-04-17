# Whiz

![Crates.io](https://img.shields.io/crates/v/whiz)

Whiz (/wɪz/) is a modern DAG/tasks runner for multi-platform monorepos.

![Demo](./demo.gif)

> Whiz is part of the
> [Metatype ecosystem](https://github.com/metatypedev/metatype). Consider
> checking out how this component integrates with the whole ecosystem and browse
> the
> [documentation](https://metatype.dev?utm_source=github&utm_medium=readme&utm_campaign=whiz)
> to see more examples.

## Getting started

```
eget zifeo/whiz --to $HOME/.local/bin
cargo install whiz --locked
cargo install --git https://github.com/zifeo/whiz --locked
```

## Usage

### passing list of values

You can pass list of values for an option by repeating their flag

```sh
whiz --arg arg1 --arg arg2
```

## Flags

| Flags               | Description                  |
| ------------------- | ---------------------------- |
| -f, --file \<FILE\> | specify the config file      |
| -h, --help          | print help information       |
| --list-jobs         | list all the available jobs  |
| -r, --run \<JOB\>   | run specific jobs            |
| -t, --timestamp     | enable timestamps in logging |
| -v, --verbose       | enable verbose mode          |
| -V, --version       | print whiz version           |

## Key bindings

### Navigation

| Keys         | Motion                              |
| ------------ | ----------------------------------- |
| l, RighArrow | go to next tab                      |
| h, LeftArrow | go to previous tab                  |
| k, Ctl + p   | scroll up one line                  |
| j, Ctl + n   | scroll down one line                |
| Ctl + u      | scroll up half page                 |
| Ctl + d      | scroll down half page               |
| Ctl + b      | scroll up full page                 |
| Ctl + f      | scroll down full page               |
| 0            | go to last tab                      |
| 1-9          | go to the tab at the given position |

### Actions

| Keys       | Motion                           |
| ---------- | -------------------------------- |
| q, Ctl + c | exit the program                 |
| r          | rerun the job in the current tab |
