use std::fs;
use std::path::Path;
use std::process::Command;

use crate::pack::{excluded, FILE_BYTE_CAP};
use crate::paths::normalize;

const PHRASES: &[&str] = &[
    "it is important to note",
    "in order to",
    "let me know if",
    "delve",
    "utilize",
    "leverage",
    "tapestry",
    "pivotal",
    "showcasing",
    "fostering",
];

pub fn noise_marks(diff: &str) -> Vec<String> {
    let mut marks = Vec::new();
    for (index, line) in diff.lines().enumerate() {
        if !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }
        let added = &line[1..];
        let line_no = index + 1;
        if added.ends_with(' ') || added.ends_with('\t') {
            marks.push(format!("line {line_no}: trailing whitespace"));
        }
        if added.contains('—') || added.contains('–') {
            marks.push(format!("line {line_no}: dash"));
        }
        if added.chars().any(|ch| matches!(ch, '“' | '”' | '‘' | '’')) {
            marks.push(format!("line {line_no}: curly quote"));
        }
        let lower = added.to_ascii_lowercase();
        for phrase in PHRASES {
            let hit = if phrase.contains(' ') {
                lower.contains(phrase)
            } else {
                word_has(&lower, phrase)
            };
            if hit {
                marks.push(format!("line {line_no}: {phrase}"));
            }
        }
    }
    marks
}

pub fn format_repo(repo: &Path, formatter: &str, files: &[String]) -> Result<(), String> {
    let files: Vec<String> = files
        .iter()
        .map(|file| normalize(file))
        .filter(|file| excluded(file).is_none())
        .filter(|file| !file.starts_with(".agentfac/"))
        .collect();
    if files.is_empty() {
        return Ok(());
    }
    if !formatter.trim().is_empty() {
        let status = Command::new("sh")
            .arg("-c")
            .arg(formatter)
            .current_dir(repo)
            .status()
            .map_err(|err| err.to_string())?;
        if !status.success() {
            return Err(format!("formatter failed: {formatter}"));
        }
        return Ok(());
    }
    let rust_files: Vec<&String> = files.iter().filter(|file| file.ends_with(".rs")).collect();
    let go_files: Vec<&String> = files.iter().filter(|file| file.ends_with(".go")).collect();
    if !rust_files.is_empty() && command_exists("rustfmt") {
        let mut cmd = Command::new("rustfmt");
        cmd.arg("--edition").arg("2021").current_dir(repo);
        for file in &rust_files {
            cmd.arg(file);
        }
        let status = cmd.status().map_err(|err| err.to_string())?;
        if !status.success() {
            return Err("rustfmt failed".into());
        }
    }
    if !go_files.is_empty() && command_exists("gofmt") {
        let mut cmd = Command::new("gofmt");
        cmd.arg("-w").current_dir(repo);
        for file in &go_files {
            cmd.arg(file);
        }
        let status = cmd.status().map_err(|err| err.to_string())?;
        if !status.success() {
            return Err("gofmt failed".into());
        }
    }
    for file in &files {
        if file.ends_with(".rs") && command_exists("rustfmt") {
            continue;
        }
        if file.ends_with(".go") && command_exists("gofmt") {
            continue;
        }
        builtin_format(repo, file)?;
    }
    Ok(())
}

fn builtin_format(repo: &Path, file: &str) -> Result<(), String> {
    let path = repo.join(file);
    if !path.is_file() {
        return Ok(());
    }
    let len = path.metadata().map(|meta| meta.len()).unwrap_or(0);
    if len > FILE_BYTE_CAP {
        return Ok(());
    }
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Ok(()),
    };
    let formatted = format_text(&text);
    if formatted != text {
        fs::write(&path, formatted).map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn format_text(text: &str) -> String {
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let newline = line.ends_with('\n');
        let body = line.trim_end_matches(['\n', '\r']).trim_end();
        out.push_str(body);
        if newline {
            out.push('\n');
        }
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn word_has(text: &str, word: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|item| item == word)
}

fn command_exists(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {name} >/dev/null 2>&1"))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
