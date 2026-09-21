use crate::config::{Status, status_path, write_status};
use crate::config::{pid_is_alive, read_existing_pid};
use crate::types::{classify, destination_for};
use crate::{build_extension_map, config, expand_tilde, log_line, move_file};
use clap::{Parser, Subcommand};
use daemonize::Daemonize;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use notify::EventKind::{Create, Modify};
use notify::event::CreateKind::File;
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, RecursiveMode, Watcher};
use signal_hook::consts::SIGHUP;
use signal_hook::flag;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

    let daemonize = Daemonize::new().pid_file(pid_path);

    if let Err(e) = daemonize.start() {
        eprintln!("failed to daemonize: {e}");
        std::process::exit(1);
    }

    let reload_flag = Arc::new(AtomicBool::new(false));
    flag::register(SIGHUP, Arc::clone(&reload_flag)).expect("failed to register SIGHUP handler");

    let mut config = match config::load_or_create_config(config_override) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            std::process::exit(1);
        }
    };

    // println!("{config:#?}");
    let mut ext_map = build_extension_map(&config);
    let mut partial: HashSet<String> =
        config.partial.extensions.iter().map(|s| s.to_lowercase()).collect();

    let mut watch_path = expand_tilde(&config.watch.path);
    let status_path = status_path().expect("Could nto determine the status path");

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
    let mut watcher = notify::recommended_watcher(tx).unwrap();
    watcher.watch(&watch_path, RecursiveMode::NonRecursive).unwrap();

    loop {
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
                if event.kind == Create(File)
                    || event.kind == Modify(ModifyKind::Name(RenameMode::Any))
                {
                    let path = event.paths.last().unwrap();

                    //prevents spamming for partial downloaded files
                    if !path.exists() {
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
            kill(Pid::from_raw(pid), Signal::SIGTERM).expect("failed to send SIGTERM");
            let _ = fs::remove_file(pid_path);
            println!("stopped dsorter (pid {pid})");
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
            kill(Pid::from_raw(pid), Signal::SIGHUP).expect("failed to send SIGHUP");
            println!("reloaded dsorter config (pid {pid})");
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
