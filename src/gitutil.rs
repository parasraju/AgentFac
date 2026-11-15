use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::pack::{excluded, FILE_BYTE_CAP, README_LINES};

pub fn toplevel(cwd: &Path) -> Result<PathBuf, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err("this directory is not a git repo. Run agentfac init.".into());
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

pub fn init(cwd: &Path) -> Result<(), String> {
    git(cwd, &["init"])?;
    Ok(())
}

pub fn branch(repo: &Path) -> String {
    git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|_| "HEAD".into())
}

pub fn origin(repo: &Path) -> Option<String> {
    git(repo, &["remote", "get-url", "origin"])
        .ok()
        .filter(|url| !url.is_empty())
}

pub fn head(repo: &Path) -> Option<String> {
    git(repo, &["rev-parse", "HEAD"]).ok()
}

pub fn status_clean(repo: &Path) -> Result<bool, String> {
    Ok(porcelain(repo)?.is_empty())
}

pub fn checkout_new(repo: &Path, name: &str) -> Result<(), String> {
    git(repo, &["checkout", "-b", name])?;
    Ok(())
}

pub fn push_head(repo: &Path) -> Result<(), String> {
    git(repo, &["push", "-u", "origin", "HEAD"])?;
    Ok(())
}

pub fn porcelain(repo: &Path) -> Result<Vec<String>, String> {
    let output = git_output(repo, &["status", "--porcelain"])?;
    Ok(parse_porcelain(&String::from_utf8_lossy(&output.stdout)))
}

pub fn diff_since(repo: &Path, start: Option<&str>) -> Result<String, String> {
    let text = match start {
        Some(sha) => git(repo, &["diff", sha]).unwrap_or_default(),
        None => git(repo, &["diff"]).unwrap_or_default(),
    };
    let mut text = text;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    let untracked = git(repo, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
    for file in untracked.lines() {
        if file.is_empty() || file.starts_with(".agentfac/") || excluded(file).is_some() {
            continue;
        }
        let path = repo.join(file);
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        if body.len() as u64 > FILE_BYTE_CAP {
            continue;
        }
        text.push_str(&format!(
            "diff --git a/{file} b/{file}\n--- /dev/null\n+++ b/{file}\n"
        ));
        for line in body.lines() {
            text.push('+');
            text.push_str(line);
            text.push('\n');
        }
    }
    Ok(text)
}

pub fn changed_names(repo: &Path, start: Option<&str>) -> Result<Vec<String>, String> {
    let listed = match start {
        Some(sha) => git(repo, &["diff", "--name-only", sha]).unwrap_or_default(),
        None => git(repo, &["diff", "--name-only"]).unwrap_or_default(),
    };
    let mut files: Vec<String> = listed
        .lines()
        .map(str::trim)
        .filter(|file| !file.is_empty())
        .map(str::to_string)
        .collect();
    let untracked = git(repo, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default();
    for file in untracked.lines() {
        if !file.is_empty() && !files.iter().any(|existing| existing == file) {
            files.push(file.to_string());
        }
    }
    files.retain(|file| !file.starts_with(".agentfac/") && excluded(file).is_none());
    Ok(files)
}

pub fn commit_paths(
    repo: &Path,
    paths: &[String],
    message: &str,
) -> Result<Option<String>, String> {
    let rels: Vec<String> = paths
        .iter()
        .map(|path| path.trim_start_matches("./").to_string())
        .filter(|path| !path.is_empty() && !path.contains(".."))
        .filter(|path| !path.starts_with(".agentfac/") && excluded(path).is_none())
        .collect();
    if rels.is_empty() {
        return Ok(None);
    }
    let mut args = vec!["add".to_string(), "--".to_string()];
    args.extend(rels);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    git(repo, &arg_refs)?;
    let staged = git(repo, &["diff", "--cached", "--name-only"]).unwrap_or_default();
    if staged.is_empty() {
        return Ok(None);
    }
    git(repo, &["commit", "-m", message])?;
    Ok(Some(git(repo, &["rev-parse", "HEAD"])?))
}

pub fn file_tree(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out.truncate(1_000);
    out
}

pub fn read_readme(root: &Path) -> Option<String> {
    for name in ["README.md", "README.rst", "README.txt", "README"] {
        let path = root.join(name);
        if path.is_file() {
            return read_limited(&path, README_LINES + 1);
        }
    }
    None
}

fn read_limited(path: &Path, lines: usize) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let mut kept = Vec::new();
    for (index, line) in std::io::BufRead::lines(reader).enumerate() {
        if index >= lines {
            break;
        }
        kept.push(line.ok()?);
    }
    Some(kept.join("\n"))
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    if out.len() >= 1_000 {
        return;
    }
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.filter_map(|entry| entry.ok()).collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if out.len() >= 1_000 {
            return;
        }
        if entry.file_name() == ".git" {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path().as_path())
            .to_string_lossy()
            .replace('\\', "/");
        let depth = rel.split('/').filter(|part| !part.is_empty()).count();
        if depth == 0 || depth > 2 {
            continue;
        }
        let file_type = entry.file_type().ok();
        let is_dir = file_type
            .as_ref()
            .map(|kind| kind.is_dir())
            .unwrap_or(false);
        let is_link = file_type
            .as_ref()
            .map(|kind| kind.is_symlink())
            .unwrap_or(false);
        if is_dir && excluded(&rel).is_some() {
            continue;
        }
        out.push(rel);
        if is_dir && !is_link && depth < 2 {
            walk(root, &entry.path(), out);
        }
    }
}

fn parse_porcelain(text: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in text.lines() {
        if line.len() < 4 {
            continue;
        }
        let path = line[3..].rsplit(" -> ").next().unwrap_or(&line[3..]);
        let path = path.trim().trim_matches('"').to_string();
        if !path.is_empty() {
            files.push(path);
        }
    }
    files
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = git_output(repo, args)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_output(repo: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), err.trim()));
    }
    Ok(output)
}
