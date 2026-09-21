use crate::error::ConfigError;
use directories::ProjectDirs;
#[cfg(unix)]
use nix::sys::signal::kill;
#[cfg(unix)]
use nix::unistd::Pid;
use std::fs;
use std::path::{Path, PathBuf};

pub fn pid_path() -> Result<PathBuf, ConfigError> {
    let proj_dirs = ProjectDirs::from("com", "rohit", "dsorter").ok_or(ConfigError::NoConfigDir)?;
    let data_dir = proj_dirs.data_dir();
    fs::create_dir_all(data_dir)?;
    Ok(data_dir.join("dsorter.pid"))
}

#[cfg(unix)]
pub fn pid_is_alive(pid: i32) -> bool {
    kill(Pid::from_raw(pid), None).is_ok()
}

#[cfg(windows)]
pub fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    unsafe {
        let handle = windows_sys::Win32::System::Threading::OpenProcess(0x1000, 0, pid as u32);
        if handle.is_null() {
            return false;
        }
        let mut exit_code = 0;
        let success =
            windows_sys::Win32::System::Threading::GetExitCodeProcess(handle, &mut exit_code) != 0;
        windows_sys::Win32::Foundation::CloseHandle(handle);
        success && exit_code == 259
    }
}

pub fn read_existing_pid(pid_path: &Path) -> Option<i32> {
    fs::read_to_string(pid_path).ok()?.trim().parse().ok()
}
