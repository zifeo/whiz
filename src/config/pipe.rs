use std::str::FromStr;

use anyhow::anyhow;
use regex::Regex;
use url::Url;

/// A pipe represents the redirection of the output of a task
/// matched by a regular expression to an [`OutputRedirection`].
#[derive(Clone)]
pub struct Pipe {
    /// Regular expression used to capture the output of the task and
    /// redirect it.
    pub regex: Regex,
    /// The place where the ouput matched by the regex is sent.
    pub redirection: OutputRedirection,
}

impl Pipe {
    /// Returns a pipe from the configuration provided.
    ///
    /// The configuration provided is a tuple of strings with the format of
    /// ([`Regex`], [`OutputRedirection`]).
    pub fn from(pipe_config: (&String, &String)) -> anyhow::Result<Self> {
        let (regex, redirection) = pipe_config;
        let regex = Regex::new(regex)?;
        let redirection = OutputRedirection::from_str(redirection)?;
        Ok(Self { regex, redirection })
    }
}

/// Set of places to which the output of a task can be redirected.
#[derive(Clone)]
pub enum OutputRedirection {
    /// Indicates that the output of a task should be sent
    /// to a new virtual tab with the given name.
    Tab(String),
    /// Indicates that the output of a task should be saved
    /// as a log file in the given path.
    File(String),
}

impl FromStr for OutputRedirection {
    type Err = anyhow::Error;

    /// Creates a new [`OutputRedirection`] from the given redirection URI.
    ///
    /// Available URI schemes:
    ///
    /// - file (default)
    /// - whiz
    ///
    /// Redirection URI examples:
    ///
    /// - whiz://virtual_views -> Tab
    /// - file:///dev/null -> File
    /// - ./logs/server.log -> File
    fn from_str(redirection_uri: &str) -> anyhow::Result<Self> {
        // URIs that do not start with a scheme are considered files by default
        if redirection_uri.starts_with('/') || redirection_uri.starts_with('.') {
            let output_redirection = OutputRedirection::File(redirection_uri.to_string());
            return Ok(output_redirection);
        }

        let redirection_uri = Url::parse(redirection_uri)?;

        let scheme = redirection_uri.scheme();
        let host = redirection_uri.host();

        let mut path = String::new();

        if let Some(host) = host {
            path += &host.to_string();
        }

        path += redirection_uri.path();

        match scheme {
            "whiz" => Ok(OutputRedirection::Tab(path)),
            "file" => Ok(OutputRedirection::File(path)),
            _ => Err(anyhow!("unsupported scheme")),
        }
    }
}
