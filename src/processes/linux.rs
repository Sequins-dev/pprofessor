use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use anyhow::Result;

use super::ProcessInfo;

pub fn list_processes() -> Result<Vec<ProcessInfo>> {
    let current_uid = unsafe { libc::geteuid() };
    let mut processes = Vec::new();

    for entry in std::fs::read_dir("/proc")? {
        let Ok(entry) = entry else {
            continue;
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.uid() != current_uid {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((name, parent_pid, start_time_micros)) = parse_stat(&stat) else {
            continue;
        };
        let executable = std::fs::read_link(entry.path().join("exe")).ok();
        let architecture = executable
            .as_deref()
            .and_then(executable_architecture)
            .unwrap_or("unknown");

        processes.push(ProcessInfo {
            pid,
            parent_pid,
            uid: metadata.uid(),
            name,
            executable_path: executable.map(|path| path.to_string_lossy().into_owned()),
            start_time_micros,
            architecture: architecture.to_string(),
            attachable: true,
            attachability_reason: None,
        });
    }

    processes.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.pid.cmp(&right.pid))
    });
    Ok(processes)
}

fn parse_stat(stat: &str) -> Option<(String, u32, u64)> {
    let name_start = stat.find('(')?;
    let name_end = stat.rfind(')')?;
    let name = stat.get(name_start + 1..name_end)?.to_string();
    let fields: Vec<&str> = stat.get(name_end + 2..)?.split_whitespace().collect();
    let parent_pid = fields.get(1)?.parse().ok()?;
    let start_ticks: u64 = fields.get(19)?.parse().ok()?;
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks_per_second <= 0 {
        return None;
    }
    let start_time_micros = start_ticks.saturating_mul(1_000_000) / ticks_per_second as u64;
    Some((name, parent_pid, start_time_micros))
}

fn executable_architecture(path: &Path) -> Option<&'static str> {
    let mut header = [0u8; 20];
    std::fs::File::open(path)
        .ok()?
        .read_exact(&mut header)
        .ok()?;
    if &header[..4] != b"\x7fELF" {
        return None;
    }
    let machine = match header[5] {
        1 => u16::from_le_bytes([header[18], header[19]]),
        2 => u16::from_be_bytes([header[18], header[19]]),
        _ => return None,
    };
    match machine {
        62 => Some("x86_64"),
        183 => Some("arm64"),
        _ => Some("unknown"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_with_spaces_and_process_identity_fields() {
        let stat = "42 (worker pool) S 7 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 12345 0";
        let (name, parent_pid, start_time) = parse_stat(stat).unwrap();
        assert_eq!(name, "worker pool");
        assert_eq!(parent_pid, 7);
        assert!(start_time > 0);
    }
}
