use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
struct HostOptions {
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    proxy_jump: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SshImportCandidate {
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
    pub source_path: String,
}

#[tauri::command]
pub fn ssh_config_scan_importable_hosts() -> Result<Vec<SshImportCandidate>, String> {
    let files = discover_ssh_config_files();
    if files.is_empty() {
        return Ok(Vec::new());
    }

    let mut candidates = Vec::new();
    let mut seen_aliases = HashSet::new();

    for path in files {
        let parsed = parse_config_file(&path)?;
        for item in parsed {
            let key = item.alias.to_lowercase();
            if seen_aliases.insert(key) {
                candidates.push(item);
            }
        }
    }

    candidates.sort_by(|a, b| a.alias.to_lowercase().cmp(&b.alias.to_lowercase()));
    Ok(candidates)
}

fn discover_ssh_config_files() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let home = match dirs::home_dir() {
        Some(v) => v,
        None => return paths,
    };

    let ssh_dir = home.join(".ssh");
    if !ssh_dir.exists() || !ssh_dir.is_dir() {
        return paths;
    }

    let main_config = ssh_dir.join("config");
    if main_config.is_file() {
        paths.push(main_config);
    }

    let mut top_level_conf = collect_conf_files(&ssh_dir);
    top_level_conf.sort();
    for p in top_level_conf {
        if !paths.contains(&p) {
            paths.push(p);
        }
    }

    let config_d_dir = ssh_dir.join("config.d");
    let mut config_d_conf = collect_conf_files(&config_d_dir);
    config_d_conf.sort();
    for p in config_d_conf {
        if !paths.contains(&p) {
            paths.push(p);
        }
    }

    paths
}

fn collect_conf_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if !dir.exists() || !dir.is_dir() {
        return files;
    }

    let entries = match fs::read_dir(dir) {
        Ok(v) => v,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if let Some(name) = p.file_name().and_then(|v| v.to_str()) {
            if name.eq_ignore_ascii_case("config") || name.ends_with(".conf") {
                files.push(p);
            }
        }
    }

    files
}

fn parse_config_file(path: &Path) -> Result<Vec<SshImportCandidate>, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let source_path = path.display().to_string();

    let mut out = Vec::new();
    let mut current_aliases: Vec<String> = Vec::new();
    let mut current_options = HostOptions::default();

    let mut flush_current = |aliases: &mut Vec<String>, opts: &mut HostOptions| {
        if aliases.is_empty() {
            return;
        }
        for alias in aliases.iter() {
            if alias.trim().is_empty() {
                continue;
            }
            let hostname = opts
                .hostname
                .clone()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| alias.clone());
            out.push(SshImportCandidate {
                alias: alias.clone(),
                hostname,
                user: opts.user.clone().unwrap_or_default(),
                port: opts.port.unwrap_or(22),
                identity_file: opts.identity_file.clone(),
                proxy_jump: opts.proxy_jump.clone(),
                source_path: source_path.clone(),
            });
        }
        aliases.clear();
        *opts = HostOptions::default();
    };

    for raw_line in content.lines() {
        let line = strip_comments(raw_line);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let key = match parts.next() {
            Some(v) => v,
            None => continue,
        };
        let rest = parts.collect::<Vec<_>>().join(" ");

        if key.eq_ignore_ascii_case("Host") {
            flush_current(&mut current_aliases, &mut current_options);
            let aliases: Vec<String> = rest
                .split_whitespace()
                .filter(|token| is_importable_alias(token))
                .map(|token| token.to_string())
                .collect();
            current_aliases = aliases;
            continue;
        }

        if key.eq_ignore_ascii_case("Match") {
            flush_current(&mut current_aliases, &mut current_options);
            continue;
        }

        if current_aliases.is_empty() {
            continue;
        }

        let value = rest.trim();
        if value.is_empty() {
            continue;
        }

        if key.eq_ignore_ascii_case("HostName") {
            current_options.hostname = Some(value.to_string());
        } else if key.eq_ignore_ascii_case("User") {
            current_options.user = Some(value.to_string());
        } else if key.eq_ignore_ascii_case("Port") {
            if let Ok(port) = value.parse::<u16>() {
                current_options.port = Some(port);
            }
        } else if key.eq_ignore_ascii_case("IdentityFile") {
            current_options.identity_file = Some(value.to_string());
        } else if key.eq_ignore_ascii_case("ProxyJump") {
            current_options.proxy_jump = Some(value.to_string());
        }
    }

    flush_current(&mut current_aliases, &mut current_options);
    Ok(out)
}

fn strip_comments(input: &str) -> String {
    let mut out = String::new();
    let mut in_single = false;
    let mut in_double = false;

    for c in input.chars() {
        if c == '\'' && !in_double {
            in_single = !in_single;
            out.push(c);
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            out.push(c);
            continue;
        }
        if c == '#' && !in_single && !in_double {
            break;
        }
        out.push(c);
    }

    out
}

fn is_importable_alias(alias: &str) -> bool {
    if alias.is_empty() || alias.starts_with('!') {
        return false;
    }
    !alias.contains('*') && !alias.contains('?') && !alias.contains('!')
}
