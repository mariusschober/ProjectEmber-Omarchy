use std::io;
use std::path::PathBuf;
use std::process::Command;

/// Result of an external command. Commands are always invoked with an argument
/// array — never through a shell — and outputs are never interpolated into
/// another command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    pub success: bool,
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutcome>;
    fn program_exists(&self, program: &str) -> bool;
}

#[derive(Debug, Default)]
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutcome> {
        let output = Command::new(program).args(args).output()?;
        Ok(CommandOutcome {
            success: output.status.success(),
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }

    fn program_exists(&self, program: &str) -> bool {
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path).any(|dir| {
            let candidate = dir.join(program);
            candidate.is_file()
                && std::fs::metadata(&candidate)
                    .map(|metadata| {
                        use std::os::unix::fs::PermissionsExt;
                        metadata.permissions().mode() & 0o111 != 0
                    })
                    .unwrap_or(false)
        })
    }
}

/// Tests use this runner to assert exact argument arrays and simulate failures.
#[derive(Debug, Default)]
pub struct RecordingRunner {
    calls: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    responses: std::sync::Mutex<std::collections::HashMap<String, CommandOutcome>>,
    programs: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl RecordingRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_program(self, name: &str) -> Self {
        self.programs.lock().unwrap().insert(name.to_string());
        self
    }

    pub fn with_response(self, program: &str, outcome: CommandOutcome) -> Self {
        self.responses
            .lock()
            .unwrap()
            .insert(program.to_string(), outcome);
        self
    }

    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for RecordingRunner {
    fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutcome> {
        self.calls.lock().unwrap().push((
            program.to_string(),
            args.iter().map(|s| s.to_string()).collect(),
        ));
        Ok(self
            .responses
            .lock()
            .unwrap()
            .get(program)
            .cloned()
            .unwrap_or(CommandOutcome {
                success: true,
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            }))
    }

    fn program_exists(&self, program: &str) -> bool {
        self.programs.lock().unwrap().contains(program)
    }
}

/// Resolves a program from PATH without invoking a shell.
pub fn resolve_program(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_runner_returns_configured_outcome() {
        let runner = RecordingRunner::new().with_response(
            "hyprctl",
            CommandOutcome {
                success: true,
                status: 0,
                stdout: "1000".into(),
                stderr: String::new(),
            },
        );
        let outcome = runner
            .run("hyprctl", &["hyprsunset", "temperature"])
            .unwrap();
        assert!(outcome.success);
        assert_eq!(outcome.stdout, "1000");
        assert_eq!(
            runner.calls(),
            vec![(
                "hyprctl".to_string(),
                vec!["hyprsunset".to_string(), "temperature".to_string()]
            )]
        );
    }
}
