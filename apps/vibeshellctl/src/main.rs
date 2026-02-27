use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use tracing::info;

use clap::{Parser, Subcommand, ValueEnum};
use swayipc::Connection;

#[derive(Debug, Parser)]
#[command(
    name = "vibeshellctl",
    about = "Control vibeshell development components"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Ask Sway to reload its configuration.
    Reload,
    /// Show whether Sway and shell components are running.
    Status,
    /// Restart a specific vibeshell component.
    Restart { component: Component },
    /// Print component logs from a captured nested-session log file.
    Logs { component: Component },
}

#[derive(Clone, Debug, ValueEnum)]
enum Component {
    Panel,
    Launcher,
    Notifd,
}

impl Component {
    fn process_name(&self) -> &'static str {
        match self {
            Self::Panel => "panel",
            Self::Launcher => "launcher",
            Self::Notifd => "notifd",
        }
    }

    fn default_start_command(&self) -> &'static str {
        match self {
            Self::Panel => "cargo run -p panel",
            Self::Launcher => "cargo run -p launcher",
            Self::Notifd => "cargo run -p notifd",
        }
    }

    fn env_command_key(&self) -> &'static str {
        match self {
            Self::Panel => "VIBESHELL_PANEL_CMD",
            Self::Launcher => "VIBESHELL_LAUNCHER_CMD",
            Self::Notifd => "VIBESHELL_NOTIFD_CMD",
        }
    }

    fn from_log_target(&self) -> &'static str {
        self.process_name()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    common::init_logging("vibeshellctl");

    let cli = Cli::parse();
    match cli.command {
        Commands::Reload => reload()?,
        Commands::Status => status()?,
        Commands::Restart { component } => restart(component)?,
        Commands::Logs { component } => logs(component)?,
    }

    Ok(())
}

fn reload() -> Result<(), Box<dyn std::error::Error>> {
    let mut connection = Connection::new()?;
    let replies = connection.run_command("reload")?;

    for reply in replies {
        if let Err(error) = reply {
            return Err(format!("sway rejected reload command: {error}").into());
        }
    }

    for component in [Component::Panel, Component::Launcher, Component::Notifd] {
        send_reload_signal(component.process_name())?;
    }

    println!("reload requested (sway + vibeshell components)");
    Ok(())
}

fn send_reload_signal(process_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("pkill")
        .args(["-HUP", "-x", process_name])
        .status()?;

    if !status.success() {
        return Err(format!("failed to send SIGHUP to {process_name}").into());
    }

    info!(process_name, "requested config reload");
    Ok(())
}

fn status() -> Result<(), Box<dyn std::error::Error>> {
    let sway_running = Connection::new().is_ok();
    println!("sway: {}", running_label(sway_running));

    for component in [Component::Panel, Component::Launcher, Component::Notifd] {
        let running = is_running(component.process_name())?;
        println!("{}: {}", component.process_name(), running_label(running));
    }

    Ok(())
}

fn restart(component: Component) -> Result<(), Box<dyn std::error::Error>> {
    let process_name = component.process_name();

    if is_running(process_name)? {
        let status = Command::new("pkill").args(["-x", process_name]).status()?;
        if !status.success() {
            return Err(format!("failed to stop {process_name}").into());
        }
        println!("stopped {process_name}");
    }

    let cmd = env::var(component.env_command_key())
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| component.default_start_command().to_owned());

    let child = Command::new("setsid")
        .args(["bash", "-lc", &cmd])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    println!("started {process_name} (pid {}) via `{cmd}`", child.id());
    Ok(())
}

fn logs(component: Component) -> Result<(), Box<dyn std::error::Error>> {
    let log_path = env::var("VIBESHELL_LOG_FILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/vibeshell-nested.log"));

    let file = fs::File::open(&log_path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to open {} ({error}). Capture logs first, e.g. `VIBESHELL_LOG=debug ./scripts/run-in-nested-sway 2>&1 | tee {}`",
                log_path.display(),
                log_path.display(),
            ),
        )
    })?;

    let target = component.from_log_target();
    let mut matched = 0usize;
    for line in io::BufReader::new(file).lines() {
        let line = line?;
        if line.contains(target) {
            println!("{line}");
            matched += 1;
        }
    }

    if matched == 0 {
        println!(
            "no log lines matched component `{}` in {}",
            component.process_name(),
            log_path.display()
        );
    }

    Ok(())
}

fn is_running(process_name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let status = Command::new("pgrep")
        .args(["-x", process_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    Ok(status.success())
}

fn running_label(running: bool) -> &'static str {
    if running {
        "running"
    } else {
        "stopped"
    }
}
