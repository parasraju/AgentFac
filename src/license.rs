use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
struct RepoRecord {
    path: String,
    origin: String,
}

pub fn home_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AGENTFAC_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".agentfac");
        }
    }
    PathBuf::from(".agentfac-home")
}

pub fn validate_key(text: &str) -> bool {
    let Some(line) = text.lines().next() else {
        return false;
    };
    let Some(rest) = line.trim().strip_prefix("AFAC-1-") else {
        return false;
    };
    let Some((payload, sum)) = rest.split_once('-') else {
        return false;
    };
    if payload.len() != 32 || !payload.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    if sum.len() != 4 || !sum.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let expect = checksum(payload);
    u32::from_str_radix(sum, 16).ok() == Some(expect)
}

pub fn generate_key() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut file) = fs::File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    }
    let payload: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("AFAC-1-{payload}-{:04x}", checksum(&payload))
}

pub fn write_key(home: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(home).map_err(|err| err.to_string())?;
    let path = home.join("license.key");
    if path.is_file() {
        let existing = fs::read_to_string(&path).unwrap_or_default();
        if validate_key(&existing) {
            return Ok(path);
        }
    }
    fs::write(&path, format!("{}\n", generate_key())).map_err(|err| err.to_string())?;
    Ok(path)
}

/// The first repo is recorded with no key. A different repo needs `license.key`.
pub fn authorize(home: &Path, repo: &Path, origin: Option<&str>) -> Result<(), String> {
    fs::create_dir_all(home).map_err(|err| err.to_string())?;
    let path = fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let path = path.to_string_lossy().to_string();
    let origin = origin.unwrap_or("").trim().to_string();
    let mut repos = read_repos(home);
    if repos
        .iter()
        .any(|record| record.path == path || (!origin.is_empty() && record.origin == origin))
    {
        if !repos.iter().any(|record| record.path == path) {
            repos.push(RepoRecord { path, origin });
            write_repos(home, &repos)?;
        }
        return Ok(());
    }
    if !repos.is_empty() && !key_ok(home) {
        let key_path = home.join("license.key");
        return Err(format!(
            "A second repo needs a valid local license key at {}. No account is used. Run `agentfac license` to write one.",
            key_path.display()
        ));
    }
    repos.push(RepoRecord { path, origin });
    write_repos(home, &repos)
}

fn key_ok(home: &Path) -> bool {
    let text = fs::read_to_string(home.join("license.key")).unwrap_or_default();
    validate_key(&text)
}

fn checksum(payload: &str) -> u32 {
    payload.bytes().map(u32::from).sum::<u32>() & 0xffff
}

fn read_repos(home: &Path) -> Vec<RepoRecord> {
    let text = fs::read_to_string(home.join("repos.txt")).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (path, origin) = line.split_once('\t').unwrap_or((line, ""));
            Some(RepoRecord {
                path: path.to_string(),
                origin: origin.to_string(),
            })
        })
        .collect()
}

fn write_repos(home: &Path, repos: &[RepoRecord]) -> Result<(), String> {
    let mut text = String::new();
    for repo in repos {
        text.push_str(&repo.path);
        text.push('\t');
        text.push_str(&repo.origin);
        text.push('\n');
    }
    fs::write(home.join("repos.txt"), text).map_err(|err| err.to_string())
}
