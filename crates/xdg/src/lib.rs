use std::collections::HashSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DesktopEntry {
    pub id: String,
    pub path: PathBuf,
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub keywords: Vec<String>,
    pub terminal: bool,
}

pub fn discover_applications() -> io::Result<Vec<DesktopEntry>> {
    let mut entries = Vec::new();
    let mut seen_ids = HashSet::new();

    for app_dir in application_dirs() {
        if !app_dir.exists() {
            continue;
        }

        for desktop_file in find_desktop_files(&app_dir)? {
            if let Some(parsed) = parse_desktop_file(&desktop_file)? {
                // Keep first by XDG search precedence.
                if seen_ids.insert(parsed.id.clone()) {
                    entries.push(parsed);
                }
            }
        }
    }

    Ok(entries)
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"));
    dirs.push(data_home.join("applications"));

    let data_dirs =
        env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".to_owned());
    for dir in data_dirs.split(':').filter(|segment| !segment.is_empty()) {
        dirs.push(Path::new(dir).join("applications"));
    }

    dirs
}

fn find_desktop_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("desktop"))
            {
                out.push(path);
            }
        }
    }

    out.sort();
    Ok(out)
}

fn parse_desktop_file(path: &Path) -> io::Result<Option<DesktopEntry>> {
    let content = fs::read_to_string(path)?;

    let mut in_desktop_entry = false;
    let mut name = None;
    let mut exec = None;
    let mut icon = None;
    let mut keywords = Vec::new();
    let mut terminal = false;
    let mut hidden = false;
    let mut no_display = false;
    let mut type_application = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }

        if !in_desktop_entry {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        let value = value.trim();

        if key.starts_with("Name[") || key.starts_with("Keywords[") {
            continue;
        }

        match key {
            "Name" => name = Some(value.to_owned()),
            "Exec" => exec = Some(value.to_owned()),
            "Icon" => icon = Some(value.to_owned()),
            "Keywords" => {
                keywords = value
                    .split(';')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            "Terminal" => terminal = value.eq_ignore_ascii_case("true"),
            "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
            "NoDisplay" => no_display = value.eq_ignore_ascii_case("true"),
            "Type" => type_application = value == "Application",
            _ => {}
        }
    }

    if hidden || no_display || !type_application {
        return Ok(None);
    }

    let (Some(name), Some(exec)) = (name, exec) else {
        return Ok(None);
    };

    Ok(Some(DesktopEntry {
        id: desktop_id(path),
        path: path.to_path_buf(),
        name,
        exec,
        icon,
        keywords,
        terminal,
    }))
}

fn desktop_id(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown.desktop")
        .to_owned()
}
