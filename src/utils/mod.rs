use crate::config::Config;
use crate::types::Category;
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn build_extension_map(config: &Config) -> HashMap<String, Category> {
    let mut map = HashMap::new();
    for (key, extensions) in &config.extensions {
        let category = Category::from_key(key);
        for ext in extensions {
            map.insert(ext.to_lowercase(), category);
        }
    }
    map
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        let home = dirs::home_dir().expect("could not determine home directory");
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

pub fn move_file(src: &Path, dest_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dest_dir)?;

    let filename = src.file_name().expect("file has no name");

    let mut dest = dest_dir.join(filename);

    // this destination might be existing already .. so we need to handle that case
    if dest.exists() {
        dest = find_unique_path(&dest);
    }

    //fails if we try to go from one kind of drive to another .. like mount points need to be same for this to work
    fs::rename(src, dest)
}

fn find_unique_path(path: &Path) -> PathBuf {
    let file_name = path.file_stem().unwrap().to_string_lossy();
    let extension = path.extension().map(|e| e.to_string_lossy().to_string());
    let parent = path.parent().unwrap();

    let mut i = 1;

    loop {
        let name = match &extension {
            Some(ext) => format!("{file_name}({i}).{ext}"),
            None => format!("{file_name}({i})"),
        };

        let candidate = parent.join(name);

        if !candidate.exists() {
            return candidate;
        }
        i += 1;
    }
}

pub fn log_line(log_path: &Path, line: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "{line}");
    }
}
