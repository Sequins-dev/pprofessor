use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::ffi::CStr;
use std::mem::{MaybeUninit, size_of};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub uid: u32,
    pub name: String,
    pub executable_path: Option<String>,
    pub start_time_micros: u64,
    pub architecture: String,
    pub attachable: bool,
    pub attachability_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Attachability {
    attachable: bool,
    reason: Option<String>,
}

const CS_OPS_STATUS: libc::c_uint = 0;
const CS_GET_TASK_ALLOW: u32 = 0x0000_0004;
const CS_RUNTIME: u32 = 0x0001_0000;
const CS_PLATFORM_BINARY: u32 = 0x0400_0000;
const SYS_CSOPS: libc::c_int = 169;

fn classify_attachability(code_signing_flags: u32) -> Attachability {
    let target_allows_debugging = code_signing_flags & CS_GET_TASK_ALLOW != 0;
    let target_is_protected = code_signing_flags & (CS_RUNTIME | CS_PLATFORM_BINARY) != 0;
    if target_is_protected && !target_allows_debugging {
        Attachability {
            attachable: false,
            reason: Some("Protected by macOS: this process does not allow debugging".to_string()),
        }
    } else {
        Attachability {
            attachable: true,
            reason: None,
        }
    }
}

fn process_attachability(pid: i32) -> Attachability {
    let mut flags = 0u32;
    let result = unsafe {
        libc::syscall(
            SYS_CSOPS,
            pid,
            CS_OPS_STATUS,
            &mut flags as *mut u32,
            size_of::<u32>(),
        )
    };
    if result == 0 {
        classify_attachability(flags)
    } else {
        Attachability {
            attachable: true,
            reason: None,
        }
    }
}

#[repr(C)]
struct ProcArchInfo {
    cpu_type: i32,
    cpu_subtype: i32,
}

const PROC_PIDARCHINFO: i32 = 19;
const CPU_ARCH_ABI64: i32 = 0x0100_0000;
const CPU_TYPE_X86: i32 = 7;
const CPU_TYPE_ARM: i32 = 12;
const CPU_TYPE_X86_64: i32 = CPU_TYPE_X86 | CPU_ARCH_ABI64;
const CPU_TYPE_ARM64: i32 = CPU_TYPE_ARM | CPU_ARCH_ABI64;

pub fn list_processes() -> Result<Vec<ProcessInfo>> {
    let uid = unsafe { libc::geteuid() };
    let mut capacity = 1024usize;
    let pids = loop {
        let mut pids = vec![0i32; capacity];
        let count = unsafe {
            libc::proc_listallpids(
                pids.as_mut_ptr().cast(),
                (pids.len() * size_of::<i32>()) as i32,
            )
        };
        if count < 0 {
            bail!(
                "proc_listallpids failed: {}",
                std::io::Error::last_os_error()
            );
        }
        if count as usize >= capacity {
            capacity *= 2;
            continue;
        }
        pids.truncate(count as usize);
        break pids;
    };

    let mut result = Vec::new();
    for pid in pids.into_iter().filter(|pid| *pid > 0) {
        let mut bsd = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                bsd.as_mut_ptr().cast(),
                size_of::<libc::proc_bsdinfo>() as i32,
            )
        };
        if size != size_of::<libc::proc_bsdinfo>() as i32 {
            continue;
        }
        let bsd = unsafe { bsd.assume_init() };
        if bsd.pbi_uid != uid {
            continue;
        }

        let name = c_char_array(&bsd.pbi_name)
            .or_else(|| c_char_array(&bsd.pbi_comm))
            .unwrap_or_else(|| pid.to_string());
        let mut path = [0u8; 4096];
        let path_len =
            unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        let executable_path = if path_len > 0 {
            CStr::from_bytes_until_nul(&path)
                .ok()
                .map(|value| value.to_string_lossy().into_owned())
        } else {
            None
        };

        let mut arch = ProcArchInfo {
            cpu_type: 0,
            cpu_subtype: 0,
        };
        let arch_size = unsafe {
            libc::proc_pidinfo(
                pid,
                PROC_PIDARCHINFO,
                0,
                (&mut arch as *mut ProcArchInfo).cast(),
                size_of::<ProcArchInfo>() as i32,
            )
        };
        let architecture = if arch_size == size_of::<ProcArchInfo>() as i32 {
            match arch.cpu_type {
                CPU_TYPE_ARM64 => "arm64",
                CPU_TYPE_X86_64 => "x86_64",
                _ => "unknown",
            }
        } else {
            "unknown"
        };

        let attachability = process_attachability(pid);
        result.push(ProcessInfo {
            pid: bsd.pbi_pid,
            parent_pid: bsd.pbi_ppid,
            uid: bsd.pbi_uid,
            name,
            executable_path,
            start_time_micros: bsd
                .pbi_start_tvsec
                .saturating_mul(1_000_000)
                .saturating_add(bsd.pbi_start_tvusec),
            architecture: architecture.to_string(),
            attachable: attachability.attachable,
            attachability_reason: attachability.reason,
        });
    }
    result.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.pid.cmp(&b.pid))
    });
    Ok(result)
}

pub fn required_helper_arch<'a>(current: &str, target: &'a str) -> Option<&'a str> {
    matches!(target, "arm64" | "x86_64")
        .then_some(target)
        .filter(|target| *target != current)
}

fn c_char_array<const N: usize>(value: &[libc::c_char; N]) -> Option<String> {
    let bytes: Vec<u8> = value
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    (!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardened_process_without_get_task_allow_is_protected() {
        let attachability = classify_attachability(CS_RUNTIME);

        assert!(!attachability.attachable);
        assert!(
            attachability
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("does not allow debugging"))
        );
    }

    #[test]
    fn platform_binary_without_get_task_allow_is_protected() {
        let attachability = classify_attachability(CS_PLATFORM_BINARY);

        assert!(!attachability.attachable);
    }

    #[test]
    fn get_task_allow_makes_hardened_process_attachable() {
        let attachability = classify_attachability(CS_RUNTIME | CS_GET_TASK_ALLOW);

        assert!(attachability.attachable);
        assert!(attachability.reason.is_none());
    }

    #[test]
    fn non_hardened_process_is_attachable() {
        let attachability = classify_attachability(0);

        assert!(attachability.attachable);
        assert!(attachability.reason.is_none());
    }
}
