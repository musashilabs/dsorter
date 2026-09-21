# dsorter

A background tool that watches a folder (like Downloads) and automatically sorts new files into type-based folders — images, docs, archives, video, audio — using cut/paste, not copy.

## Install

```
cargo install --path .
```

## Quick start

```
dsorter start
```

This starts dsorter in the background. It watches your configured folder and moves new files as they show up.

```
dsorter stop
```

Stops it.

```
dsorter status
```

Shows if it's running, what folder it's watching, and how many files it's sorted so far.

```
dsorter log --tail 20
dsorter log --head 20
```

Shows recent log entries.

## Config

The first time you run `dsorter start`, it creates a config file for you at:

- macOS: `~/Library/Application Support/com.rohit.dsorter/config.toml`
- Linux: `~/.config/dsorter/config.toml`

Open it and edit it. It looks like this:

```toml
[watch]
path = "~/Downloads"

[destinations]
image = "~/Pictures/Downloads-Sorted"
doc = "~/Documents/Downloads-Sorted"
archive = "~/Downloads-Sorted/Archives"
video = "~/Movies/Downloads-Sorted"
audio = "~/Music/Downloads-Sorted"
other = "~/Downloads-Sorted/Other"

[extensions]
image = ["jpg", "png", "gif", "jpeg", "heif"]
doc = ["pdf", "docx", "txt", "xlsx"]
archive = ["zip", "tar", "rar"]
video = ["mkv", "mp4", "mov"]
audio = ["mp3", "wav", "mp4a"]

[partial]
extensions = ["crdownload", "part", "download"]
```

- `watch.path` — the folder to watch
- `destinations` — where each file type gets moved to
- `extensions` — which file extensions belong to which type
- `partial.extensions` — files with these extensions are skipped (they're still downloading)

You can change any of this and reload it without restarting:

```
dsorter reload
```

If you change `watch.path` to a different folder, that also takes effect right away — no restart needed.

## Testing changes safely

```
dsorter start --dry-run
```

This runs in the foreground and logs what it *would* move, without actually moving anything. Good for checking your config before trusting it with real files.

## Custom config location

```
dsorter --config /path/to/other.toml start
```

Points dsorter at a config file somewhere else instead of the default location.

## Run on startup

```
dsorter install
```

Sets up dsorter to start automatically when you log in (`launchd` on macOS, `systemd` on Linux). It'll print the exact command to run next to turn it on.

## If two files have the same name

If a file with the same name already exists at the destination, dsorter renames the new one instead of overwriting — e.g. `report.pdf` becomes `report(1).pdf`.

## Tech

- Rust
- `notify` for watching the filesystem
- `clap` for the command-line interface

## Status

Working. macOS tested, Linux support built but not yet verified.