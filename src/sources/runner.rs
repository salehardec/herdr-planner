//! Non-blocking command runner with a timeout: one tokio process per call.

use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct Output {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.status == 0
    }

    /// The error text a failed run shows: stderr, else the exit code.
    pub fn error(&self) -> String {
        let e = self.stderr.trim();
        if e.is_empty() {
            format!("exit {}", self.status)
        } else {
            e.to_string()
        }
    }
}

/// Runs `argv` with `stdin` (optional) and returns its output, or an error text
/// when it could not start or exceeded `timeout`.
pub async fn run(
    argv: &[String],
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<Output, String> {
    if argv.is_empty() {
        return Err("пустая команда".into());
    }
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("не запустилась команда {}: {}", argv[0], e))?;
    if let Some(text) = stdin {
        if let Some(mut pipe) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = pipe.write_all(text.as_bytes()).await;
            let _ = pipe.shutdown().await;
        }
    }
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => Ok(Output {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }),
        Ok(Err(e)) => Err(format!("{}: {}", argv[0], e)),
        Err(_) => Err(format!(
            "{} не ответила за {} с",
            argv[0],
            timeout.as_secs()
        )),
    }
}

pub fn argv<I: IntoIterator<Item = S>, S: AsRef<str>>(parts: I) -> Vec<String> {
    parts.into_iter().map(|s| s.as_ref().to_string()).collect()
}
