use clap::Parser;
use dsorter::cli::{
    Cli, Commands, handle_reload, handle_start, handle_status, handle_stop, print_log_head,
    print_log_tail,
};
use dsorter::config;
use notify::Result;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let log_path = config::log_path().expect("Could not determine the log path");
    let pid_path = config::pid_path().expect("Could not determine the pid path");
    let status_path = config::status_path().expect("Could not determine the status path");

    match cli.command {
        Some(Commands::Log { head, tail }) => {
            if let Some(n) = head {
                print_log_head(&log_path, n);
            } else if let Some(n) = tail {
                print_log_tail(&log_path, n);
            } else {
                eprintln!("specify --head <N> or --tail <N>");
            }
            return Ok(());
        }

        Some(Commands::Start) => handle_start(&pid_path, &log_path),

        Some(Commands::Stop) => handle_stop(&pid_path),

        Some(Commands::Status) => handle_status(&pid_path, &status_path),

        Some(Commands::Reload) => handle_reload(&pid_path),

        None => {
            println!("dsorter — watches a folder and auto-sorts new files by type\n");
            println!("USAGE:");
            println!("  dsorter start           Start watching (daemonizes)");
            println!("  dsorter stop            Stop the running daemon");
            println!("  dsorter status          Show current daemon status");
            println!("  dsorter reload          Reload config without restarting");
            println!("  dsorter log --tail N    Show last N log lines");
            println!("  dsorter log --head N    Show first N log lines");
            println!("\nRun `dsorter --help` for full details.");
        }
    }

    Ok(())
}
