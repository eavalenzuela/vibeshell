use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
struct BindingConfig {
    output: PathBuf,
    launcher_toggle_key: String,
    launcher_toggle_command: String,
    screenshot_key: String,
    screenshot_command: String,
    volume_up_key: String,
    volume_up_command: String,
    volume_down_key: String,
    volume_down_command: String,
    volume_mute_key: String,
    volume_mute_command: String,
    brightness_up_key: String,
    brightness_up_command: String,
    brightness_down_key: String,
    brightness_down_command: String,
    shell_quit_key: String,
    shell_quit_command: String,
    shell_restart_key: String,
    shell_restart_command: String,
    zoom_in_mode_key: String,
    zoom_in_mode_command: String,
    zoom_out_mode_key: String,
    zoom_out_mode_command: String,
    cycle_strip_forward_key: String,
    cycle_strip_forward_command: String,
    cycle_strip_backward_key: String,
    cycle_strip_backward_command: String,
    cycle_cluster_forward_key: String,
    cycle_cluster_forward_command: String,
    cycle_cluster_backward_key: String,
    cycle_cluster_backward_command: String,
}

impl Default for BindingConfig {
    fn default() -> Self {
        Self {
            output: PathBuf::from("dev/sway.bindings.generated"),
            launcher_toggle_key: "$mod+space".to_owned(),
            launcher_toggle_command:
                "swaymsg '[app_id=\"com.vibeshell.launcher\"] kill' || launcher".to_owned(),
            screenshot_key: "Print".to_owned(),
            screenshot_command: "grim -g \"$(slurp)\" - | wl-copy".to_owned(),
            volume_up_key: "XF86AudioRaiseVolume".to_owned(),
            volume_up_command: "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+".to_owned(),
            volume_down_key: "XF86AudioLowerVolume".to_owned(),
            volume_down_command: "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-".to_owned(),
            volume_mute_key: "XF86AudioMute".to_owned(),
            volume_mute_command: "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle".to_owned(),
            brightness_up_key: "XF86MonBrightnessUp".to_owned(),
            brightness_up_command: "brightnessctl set +10%".to_owned(),
            brightness_down_key: "XF86MonBrightnessDown".to_owned(),
            brightness_down_command: "brightnessctl set 10%-".to_owned(),
            shell_quit_key: "$mod+Shift+e".to_owned(),
            shell_quit_command: "swaymsg exit".to_owned(),
            shell_restart_key: "$mod+Shift+r".to_owned(),
            shell_restart_command: "swaymsg reload".to_owned(),
            zoom_in_mode_key: "$mod+equal".to_owned(),
            zoom_in_mode_command: "vibeshellctl ipc zoom-in-mode".to_owned(),
            zoom_out_mode_key: "$mod+minus".to_owned(),
            zoom_out_mode_command: "vibeshellctl ipc zoom-out-mode".to_owned(),
            cycle_strip_forward_key: "$mod+period".to_owned(),
            cycle_strip_forward_command: "vibeshellctl ipc cycle-strip-forward".to_owned(),
            cycle_strip_backward_key: "$mod+comma".to_owned(),
            cycle_strip_backward_command: "vibeshellctl ipc cycle-strip-backward".to_owned(),
            cycle_cluster_forward_key: "$mod+Tab".to_owned(),
            cycle_cluster_forward_command: "vibeshellctl ipc cycle-cluster --direction forward"
                .to_owned(),
            cycle_cluster_backward_key: "$mod+Shift+Tab".to_owned(),
            cycle_cluster_backward_command: "vibeshellctl ipc cycle-cluster --direction backward"
                .to_owned(),
        }
    }
}

fn parse_args() -> Result<BindingConfig, String> {
    let mut config = BindingConfig::default();
    let mut args = env::args().skip(1);

    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;

        match flag.as_str() {
            "--output" => config.output = PathBuf::from(value),
            "--launcher-toggle-key" => config.launcher_toggle_key = value,
            "--launcher-toggle-command" => config.launcher_toggle_command = value,
            "--screenshot-key" => config.screenshot_key = value,
            "--screenshot-command" => config.screenshot_command = value,
            "--volume-up-key" => config.volume_up_key = value,
            "--volume-up-command" => config.volume_up_command = value,
            "--volume-down-key" => config.volume_down_key = value,
            "--volume-down-command" => config.volume_down_command = value,
            "--volume-mute-key" => config.volume_mute_key = value,
            "--volume-mute-command" => config.volume_mute_command = value,
            "--brightness-up-key" => config.brightness_up_key = value,
            "--brightness-up-command" => config.brightness_up_command = value,
            "--brightness-down-key" => config.brightness_down_key = value,
            "--brightness-down-command" => config.brightness_down_command = value,
            "--shell-quit-key" => config.shell_quit_key = value,
            "--shell-quit-command" => config.shell_quit_command = value,
            "--shell-restart-key" => config.shell_restart_key = value,
            "--shell-restart-command" => config.shell_restart_command = value,
            "--zoom-in-mode-key" => config.zoom_in_mode_key = value,
            "--zoom-in-mode-command" => config.zoom_in_mode_command = value,
            "--zoom-out-mode-key" => config.zoom_out_mode_key = value,
            "--zoom-out-mode-command" => config.zoom_out_mode_command = value,
            "--cycle-strip-forward-key" => config.cycle_strip_forward_key = value,
            "--cycle-strip-forward-command" => config.cycle_strip_forward_command = value,
            "--cycle-strip-backward-key" => config.cycle_strip_backward_key = value,
            "--cycle-strip-backward-command" => config.cycle_strip_backward_command = value,
            "--cycle-cluster-forward-key" => config.cycle_cluster_forward_key = value,
            "--cycle-cluster-forward-command" => config.cycle_cluster_forward_command = value,
            "--cycle-cluster-backward-key" => config.cycle_cluster_backward_key = value,
            "--cycle-cluster-backward-command" => config.cycle_cluster_backward_command = value,
            "--help" | "-h" => return Err(help_text()),
            _ => return Err(format!("unknown argument: {flag}\n\n{}", help_text())),
        }
    }

    Ok(config)
}

fn help_text() -> String {
    [
        "Usage: cargo run -p sway --bin generate-bindings -- [OPTIONS]",
        "",
        "Options:",
        "  --output <path>",
        "  --launcher-toggle-key <key>",
        "  --launcher-toggle-command <command>",
        "  --screenshot-key <key>",
        "  --screenshot-command <command>",
        "  --volume-up-key <key>",
        "  --volume-up-command <command>",
        "  --volume-down-key <key>",
        "  --volume-down-command <command>",
        "  --volume-mute-key <key>",
        "  --volume-mute-command <command>",
        "  --brightness-up-key <key>",
        "  --brightness-up-command <command>",
        "  --brightness-down-key <key>",
        "  --brightness-down-command <command>",
        "  --shell-quit-key <key>",
        "  --shell-quit-command <command>",
        "  --shell-restart-key <key>",
        "  --shell-restart-command <command>",
        "  --zoom-in-mode-key <key>",
        "  --zoom-in-mode-command <command>",
        "  --zoom-out-mode-key <key>",
        "  --zoom-out-mode-command <command>",
        "  --cycle-strip-forward-key <key>",
        "  --cycle-strip-forward-command <command>",
        "  --cycle-strip-backward-key <key>",
        "  --cycle-strip-backward-command <command>",
        "  --cycle-cluster-forward-key <key>",
        "  --cycle-cluster-forward-command <command>",
        "  --cycle-cluster-backward-key <key>",
        "  --cycle-cluster-backward-command <command>",
    ]
    .join("\n")
}

fn render(config: &BindingConfig) -> String {
    [
        "# This file is generated by `cargo run -p sway --bin generate-bindings -- ...`.",
        "# Do not edit manually.",
        "",
        &format!(
            "bindsym {} exec {}",
            config.launcher_toggle_key, config.launcher_toggle_command
        ),
        &format!(
            "bindsym {} exec {}",
            config.screenshot_key, config.screenshot_command
        ),
        "",
        &format!(
            "bindsym {} exec {}",
            config.volume_up_key, config.volume_up_command
        ),
        &format!(
            "bindsym {} exec {}",
            config.volume_down_key, config.volume_down_command
        ),
        &format!(
            "bindsym {} exec {}",
            config.volume_mute_key, config.volume_mute_command
        ),
        "",
        &format!(
            "bindsym {} exec {}",
            config.brightness_up_key, config.brightness_up_command
        ),
        &format!(
            "bindsym {} exec {}",
            config.brightness_down_key, config.brightness_down_command
        ),
        "",
        &format!(
            "bindsym {} exec {}",
            config.zoom_in_mode_key, config.zoom_in_mode_command
        ),
        &format!(
            "bindsym {} exec {}",
            config.zoom_out_mode_key, config.zoom_out_mode_command
        ),
        &format!(
            "bindsym {} exec {}",
            config.cycle_strip_forward_key, config.cycle_strip_forward_command
        ),
        &format!(
            "bindsym {} exec {}",
            config.cycle_strip_backward_key, config.cycle_strip_backward_command
        ),
        &format!(
            "bindsym {} exec {}",
            config.cycle_cluster_forward_key, config.cycle_cluster_forward_command
        ),
        &format!(
            "bindsym {} exec {}",
            config.cycle_cluster_backward_key, config.cycle_cluster_backward_command
        ),
        "",
        &format!(
            "bindsym {} exec {}",
            config.shell_quit_key, config.shell_quit_command
        ),
        &format!(
            "bindsym {} exec {}",
            config.shell_restart_key, config.shell_restart_command
        ),
        "",
    ]
    .join("\n")
}

fn run() -> Result<(), String> {
    let config = parse_args()?;
    let contents = render(&config);

    if let Some(parent) = config.output.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create output directory {}: {error}",
                parent.display()
            )
        })?;
    }

    fs::write(&config.output, contents)
        .map_err(|error| format!("failed writing {}: {error}", config.output.display()))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_output_is_deterministic() {
        let config = BindingConfig::default();
        let first = render(&config);
        let second = render(&config);

        assert_eq!(first, second);
        assert!(first.contains("bindsym $mod+space exec"));
        assert!(first.contains("bindsym XF86AudioRaiseVolume exec"));
        assert!(first.contains("bindsym $mod+equal exec vibeshellctl ipc zoom-in-mode"));
        assert!(first.contains("bindsym $mod+period exec vibeshellctl ipc cycle-strip-forward"));
        assert!(first.contains("bindsym $mod+Tab exec vibeshellctl ipc cycle-cluster"));
        assert!(first.contains("bindsym $mod+Shift+r exec"));
    }
}
