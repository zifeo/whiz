# Whiz

Modern [DAG](https://en.wikipedia.org/wiki/Directed_acyclic_graph)/tasks runner.

## Getting started

```
cargo install --git https://github.com/zifeo/whiz --locked
```

## Usage

### passing list of values

You can pass list of values for an option by repeating their flag

```sh
whiz --arg arg1 --arg arg2
```

## Flags

| Flags             | Description                  |
| ----------------- | ---------------------------- |
| -f, --file        | specify the config file      |
| -h, --help        | print help information       |
| -r, --run \<JOB\> | run specific jobs            |
| -t, --timestamp   | enable timestamps in logging |
| -v, --verbose     | enable verbose mode          |
| -V, --version     | print whiz version           |

## Key bindings

### navigation

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

### actions

| Keys       | Motion                           |
| ---------- | -------------------------------- |
| q, Ctl + c | exit the program                 |
| r          | rerun the job in the current tab |
