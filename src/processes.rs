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
