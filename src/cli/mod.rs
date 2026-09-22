use crate::config::{Status, pid_is_alive, read_existing_pid, status_path, write_status};
use crate::types::{classify, destination_for};
use crate::{build_extension_map, config, expand_tilde, log_line, move_file};
use clap::{Parser, Subcommand};
use notify::EventKind::{Create, Modify};
use notify::event::ModifyKind;
use notify::{Event, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use daemonize::Daemonize;
#[cfg(unix)]
use nix::sys::signal::{Signal, kill};
#[cfg(unix)]
use nix::unistd::Pid;
#[cfg(unix)]
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
#[cfg(unix)]
use signal_hook::flag;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "dsorter", about = "Watches a folder and auto-sorts new files by type")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start watching and sorting (daemonizes)
    Start {
        /// Classify and log without actually moving files
        #[arg(long)]
        dry_run: bool,
    },
    /// Stop the running daemon
    Stop,
    /// View recent log entries
    Log {
        /// Show the first N log lines
        #[arg(long)]
        head: Option<usize>,
        /// Show the last N log lines
        #[arg(long)]
        tail: Option<usize>,
    },

    ///Show current daemon status
    Status,

    /// Reload config without restarting
    Reload,

    /// Install a launchd/systemd unit to run dsorter on login/boot
    Install,
}

pub fn print_log_head(log_path: &Path, n: usize) {
    let contents = fs::read_to_string(log_path).unwrap_or_default();
    for line in contents.lines().take(n) {
        println!("{line}");
    }
}

pub fn print_log_tail(log_path: &Path, n: usize) {
    let contents = fs::read_to_string(log_path).unwrap_or_default();
    let lines: Vec<&str> = contents.lines().collect();
    let start = lines.len().saturating_sub(n);
    for line in &lines[start..] {
        println!("{line}");
    }
}

pub fn handle_start(
    pid_path: &Path,
    log_path: &Path,
    config_override: Option<&Path>,
    dry_run: bool,
) {
    if let Some(pid) = read_existing_pid(pid_path) {
        if pid_is_alive(pid) {
            eprintln!("dsorter is already running (pid {pid})");
            std::process::exit(1);
        } else {
            let _ = fs::remove_file(pid_path); // stale pidfile, clean it up
        }
    }

    #[cfg(windows)]
    if std::env::var_os("DSORTER_DAEMON").is_none() {
        let exe = std::env::current_exe().expect("could not resolve current exe");
        let mut command = Command::new(exe);
        if let Some(config_path) = config_override {
            command.arg("--config").arg(config_path);
        }
        command.arg("start");
        if dry_run {
            command.arg("--dry-run");
        }
        command.env("DSORTER_DAEMON", "1");
        command.creation_flags(0x00000008 | 0x00000200);
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        let child = command.spawn().expect("failed to spawn background process");
        println!("started dsorter in background (pid {})", child.id());
        std::process::exit(0);
    }

    #[cfg(windows)]
    fs::write(pid_path, std::process::id().to_string()).expect("failed to write pid file");

    #[cfg(unix)]
    {
        let daemonize = Daemonize::new().pid_file(pid_path);
        if let Err(e) = daemonize.start() {
            eprintln!("failed to daemonize: {e}");
            std::process::exit(1);
        }
    }

    let reload_flag = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::new(AtomicBool::new(false));

    #[cfg(unix)]
    {
        flag::register(SIGHUP, Arc::clone(&reload_flag))
            .expect("failed to register SIGHUP handler");
        flag::register(SIGTERM, Arc::clone(&stop_flag))
            .expect("failed to register SIGTERM handler");
        flag::register(SIGINT, Arc::clone(&stop_flag)).expect("failed to register SIGINT handler");
    }

    #[cfg(windows)]
    let (_stop_event, _reload_event) = {
        let stop_name: Vec<u16> = "Local\\dsorter_stop\0".encode_utf16().collect();
        let reload_name: Vec<u16> = "Local\\dsorter_reload\0".encode_utf16().collect();

        unsafe {
            let stop_handle = windows_sys::Win32::System::Threading::CreateEventW(
                std::ptr::null(),
                0,
                0,
                stop_name.as_ptr(),
            );
            windows_sys::Win32::System::Threading::ResetEvent(stop_handle);
            let reload_handle = windows_sys::Win32::System::Threading::CreateEventW(
                std::ptr::null(),
                0,
                0,
                reload_name.as_ptr(),
            );
            windows_sys::Win32::System::Threading::ResetEvent(reload_handle);

            let reload_flag = Arc::clone(&reload_flag);
            let stop_flag = Arc::clone(&stop_flag);
            let stop_raw = stop_handle as isize;
            let reload_raw = reload_handle as isize;

            std::thread::spawn(move || {
                let handles = [stop_raw as _, reload_raw as _];
                const INFINITE: u32 = 0xFFFFFFFF;
                const WAIT_OBJECT_0: u32 = windows_sys::Win32::Foundation::WAIT_OBJECT_0;
                loop {
                    let wait_result = windows_sys::Win32::System::Threading::WaitForMultipleObjects(
                        2,
                        handles.as_ptr(),
                        0,
                        INFINITE,
                    );
                    if wait_result == WAIT_OBJECT_0 {
                        stop_flag.store(true, Ordering::Relaxed);
                        break;
                    } else if wait_result == WAIT_OBJECT_0 + 1 {
                        reload_flag.store(true, Ordering::Relaxed);
                    } else {
                        break;
                    }
                }
            });

            (stop_handle, reload_handle)
        }
    };

    let mut config = match config::load_or_create_config(config_override) {
        Ok(c) => c,
        Err(e) => {
            log_line(log_path, &format!("config error: {e}"));
            std::process::exit(1);
        }
    };

    // println!("{config:#?}");
    let mut ext_map = build_extension_map(&config);
    let mut partial: HashSet<String> =
        config.partial.extensions.iter().map(|s| s.to_lowercase()).collect();

    let mut watch_path = expand_tilde(&config.watch.path);
    let status_path = status_path().expect("Could not determine the status path");

    let started_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let mut status = Status {
        watching: watch_path.display().to_string(),
        pid: std::process::id() as i32,
        started_at,
        last_action: None,
        files_sorted: 0,
    };
    write_status(&status_path, &status);

    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = match notify::recommended_watcher(tx) {
        Ok(w) => w,
        Err(e) => {
            log_line(log_path, &format!("failed to create watcher: {e}"));
            std::process::exit(1);
        }
    };

    if let Err(e) = watcher.watch(&watch_path, RecursiveMode::NonRecursive) {
        log_line(log_path, &format!("failed to watch {}: {e}", watch_path.display()));
        std::process::exit(1);
    }

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            log_line(log_path, "dsorter received stop signal, shutting down");
            let _ = fs::remove_file(pid_path);
            break;
        }

        if reload_flag.swap(false, Ordering::Relaxed) {
            match config::load_or_create_config(config_override) {
                Ok(new_config) => {
                    let new_watch_path = expand_tilde(&new_config.watch.path);
                    if new_watch_path != watch_path {
                        if let Err(e) = watcher.unwatch(&watch_path) {
                            log_line(
                                log_path,
                                &format!("failed to unwatch {}: {e}", watch_path.display()),
                            );
                        }
                        match watcher.watch(&new_watch_path, RecursiveMode::NonRecursive) {
                            Ok(()) => {
                                log_line(
                                    log_path,
                                    &format!("now watching {}", new_watch_path.display()),
                                );
                                watch_path = new_watch_path;
                                status.watching = watch_path.display().to_string();
                                write_status(&status_path, &status);
                            }
                            Err(e) => {
                                // new path failed — fall back to the old one
                                log_line(
                                    log_path,
                                    &format!(
                                        "failed to watch {}: {e} — reverting",
                                        new_watch_path.display()
                                    ),
                                );
                                let _ = watcher.watch(&watch_path, RecursiveMode::NonRecursive);
                            }
                        }
                    }

                    config = new_config;
                    ext_map = build_extension_map(&config);
                    partial = config.partial.extensions.iter().map(|s| s.to_lowercase()).collect();
                    log_line(log_path, "config reloaded");
                }
                Err(e) => log_line(log_path, &format!("reload failed, keeping old config: {e}")),
            }
        }

        let res = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(res) => res,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                log_line(log_path, "watcher channel disconnected, shutting down");
                break;
            }
        };

        match res {
            Ok(event) => {
                let is_candidate = matches!(event.kind, Create(_) | Modify(ModifyKind::Name(_)));
                if is_candidate {
                    let Some(path) = event.paths.last() else {
                        continue;
                    };

                    //prevents spamming for partial downloaded files
                    if !path.exists() || path.is_dir() {
                        continue;
                    }
                    let filename = path.file_name().unwrap().to_string_lossy().to_string();
                    let extension = path.extension().and_then(|e| e.to_str());

                    if let Some(ext) = extension
                        && partial.contains(&ext.to_lowercase())
                    {
                        continue;
                    }

                    let category = classify(extension, &ext_map);
                    log_line(log_path, &format!("{filename} -> {category:?}"));

                    if let Some(dest_dir) = destination_for(category, &config) {
                        if dry_run {
                            log_line(
                                log_path,
                                &format!(
                                    "[dry-run] would move {filename} -> {}",
                                    dest_dir.display()
                                ),
                            );
                        } else {
                            match move_file(path, &dest_dir) {
                                Ok(()) => {
                                    log_line(
                                        log_path,
                                        &format!("moved {filename} -> {}", dest_dir.display()),
                                    );
                                    status.files_sorted += 1;
                                    status.last_action =
                                        Some(format!("moved {filename} -> {}", dest_dir.display()));
                                    write_status(&status_path, &status);
                                }
                                Err(e) => {
                                    log_line(log_path, &format!("failed to move {filename}: {e}"))
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => log_line(log_path, &format!("watch error: {e}")),
        }
    }
}

pub fn handle_stop(pid_path: &Path) {
    match read_existing_pid(pid_path) {
        Some(pid) if pid_is_alive(pid) => {
            #[cfg(unix)]
            {
                kill(Pid::from_raw(pid), Signal::SIGTERM).expect("failed to send SIGTERM");
                let _ = fs::remove_file(pid_path);
                println!("stopped dsorter (pid {pid})");
            }
            #[cfg(windows)]
            {
                let event_name: Vec<u16> = "Local\\dsorter_stop\0".encode_utf16().collect();
                unsafe {
                    let handle = windows_sys::Win32::System::Threading::OpenEventW(
                        0x0002,
                        0,
                        event_name.as_ptr(),
                    );
                    if !handle.is_null() {
                        windows_sys::Win32::System::Threading::SetEvent(handle);
                        windows_sys::Win32::Foundation::CloseHandle(handle);
                    }
                }
                for _ in 0..30 {
                    if !pid_is_alive(pid) {
                        let _ = fs::remove_file(pid_path);
                        println!("stopped dsorter (pid {pid})");
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                unsafe {
                    let process_handle =
                        windows_sys::Win32::System::Threading::OpenProcess(0x0001, 0, pid as u32);
                    if !process_handle.is_null() {
                        windows_sys::Win32::System::Threading::TerminateProcess(process_handle, 1);
                        windows_sys::Win32::Foundation::CloseHandle(process_handle);
                    }
                }
                let _ = fs::remove_file(pid_path);
                println!("stopped dsorter (pid {pid})");
            }
        }
        _ => {
            eprintln!("dsorter is not running");
        }
    }
}

pub fn handle_status(pid_path: &Path, status_path: &Path) {
    match read_existing_pid(pid_path) {
        Some(pid) if pid_is_alive(pid) => match fs::read_to_string(status_path) {
            Ok(json) => match serde_json::from_str::<Status>(&json) {
                Ok(s) => {
                    println!("dsorter is running (pid {})", s.pid);
                    println!("watching:     {}", s.watching);
                    println!("files sorted: {}", s.files_sorted);
                    if let Some(last) = &s.last_action {
                        println!("last action:  {last}");
                    }
                }
                Err(_) => println!("dsorter is running (pid {pid}) — status file unreadable"),
            },
            Err(_) => println!("dsorter is running (pid {pid}) — no status file yet"),
        },
        _ => println!("dsorter is not running"),
    }
}

pub fn handle_reload(pid_path: &Path) {
    match read_existing_pid(pid_path) {
        Some(pid) if pid_is_alive(pid) => {
            #[cfg(unix)]
            {
                kill(Pid::from_raw(pid), Signal::SIGHUP).expect("failed to send SIGHUP");
                println!("reloaded dsorter config (pid {pid})");
            }
            #[cfg(windows)]
            {
                let event_name: Vec<u16> = "Local\\dsorter_reload\0".encode_utf16().collect();
                unsafe {
                    let handle = windows_sys::Win32::System::Threading::OpenEventW(
                        0x0002,
                        0,
                        event_name.as_ptr(),
                    );
                    if !handle.is_null() {
                        windows_sys::Win32::System::Threading::SetEvent(handle);
                        windows_sys::Win32::Foundation::CloseHandle(handle);
                    }
                }
                println!("reloaded dsorter config (pid {pid})");
            }
        }
        _ => eprintln!("dsorter is not running"),
    }
}

#[cfg(target_os = "macos")]
pub fn handle_install() {
    let exe = std::env::current_exe().expect("could not resolve binary path");
    let home = dirs::home_dir().expect("no home dir");
    let plist_path = home.join("Library/LaunchAgents/com.rohit.dsorter.plist");

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.rohit.dsorter</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>start</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>
"#,
        exe.display()
    );

    fs::write(&plist_path, plist).expect("failed to write plist");
    println!("wrote {}", plist_path.display());
    println!("run: launchctl load {}", plist_path.display());
}

#[cfg(target_os = "linux")]
pub fn handle_install() {
    let exe = std::env::current_exe().expect("could not resolve binary path");
    let home = dirs::home_dir().expect("no home dir");
    let unit_dir = home.join(".config/systemd/user");
    fs::create_dir_all(&unit_dir).expect("failed to create systemd user dir");
    let unit_path = unit_dir.join("dsorter.service");

    let unit = format!(
        r#"[Unit]
Description=dsorter file watcher

[Service]
ExecStart={} start
Restart=on-failure

[Install]
WantedBy=default.target
"#,
        exe.display()
    );

    fs::write(&unit_path, unit).expect("failed to write unit file");
    println!("wrote {}", unit_path.display());
    println!("run: systemctl --user enable --now dsorter.service");
}

#[cfg(target_os = "windows")]
pub fn handle_install() {
    let exe = std::env::current_exe().expect("could not resolve binary path");
    let command = format!("\"{}\" start", exe.display());

    let status = Command::new("reg")
        .args([
            "add",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "dsorter",
            "/t",
            "REG_SZ",
            "/d",
            &command,
            "/f",
        ])
        .status()
        .expect("failed to execute reg.exe");

    if status.success() {
        println!(r"Registered dsorter in HKCU\Software\Microsoft\Windows\CurrentVersion\Run");
        println!("dsorter will start automatically when you log in.");
    } else {
        eprintln!("failed to register auto-start: reg.exe exited with code {status}");
        std::process::exit(1);
    }
}
