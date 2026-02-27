use std::process::Command;

use super::StatusProvider;

#[derive(Clone, Debug)]
pub struct AudioStatus {
    pub volume_percent: u8,
    pub muted: bool,
}

pub struct AudioProvider;

impl StatusProvider for AudioProvider {
    type Output = AudioStatus;

    fn poll(&self) -> Option<Self::Output> {
        let output = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8(output.stdout).ok()?;
        parse_wpctl_output(&stdout)
    }
}

fn parse_wpctl_output(raw: &str) -> Option<AudioStatus> {
    let mut tokens = raw.split_whitespace();
    let _label = tokens.next()?;
    let volume = tokens.next()?.parse::<f32>().ok()?;
    let muted = tokens.any(|token| token.eq_ignore_ascii_case("[MUTED]"));
    let volume_percent = (volume * 100.0).round().clamp(0.0, 100.0) as u8;

    Some(AudioStatus {
        volume_percent,
        muted,
    })
}
