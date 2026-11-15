use std::fs;
use std::path::{Path, PathBuf};

use crate::pack::SKILL_CAP;
use crate::text::{char_count, prefix_chars};

pub fn find_skill(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    for dir in dirs {
        let markdown = dir.join(format!("{name}.md"));
        if markdown.is_file() {
            return Some(markdown);
        }
        let skill = dir.join(name).join("SKILL.md");
        if skill.is_file() {
            return Some(skill);
        }
    }
    None
}

pub fn load_skill(
    cache: &Path,
    dirs: &[PathBuf],
    name: &str,
) -> Result<(String, Vec<String>), String> {
    let path = find_skill(dirs, name).ok_or_else(|| {
        format!("skill file not found: {name}. Set skills_dir in .agentfac/config.toml")
    })?;
    let raw = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    let hash = sha256(&raw);
    let cached = cache.join(format!("{name}.txt"));
    let cached_hash = cache.join(format!("{name}.sha256"));
    if cached.is_file() && cached_hash.is_file() {
        let stored = fs::read_to_string(&cached_hash).unwrap_or_default();
        if stored.trim() == hash {
            let text = fs::read_to_string(&cached).map_err(|err| err.to_string())?;
            return Ok((text, vec![format!("skill cache hit: {name}")]));
        }
    }
    let (text, note) = trim_skill(&raw, SKILL_CAP);
    fs::create_dir_all(cache).map_err(|err| err.to_string())?;
    fs::write(&cached, &text).map_err(|err| err.to_string())?;
    fs::write(&cached_hash, format!("{hash}\n")).map_err(|err| err.to_string())?;
    let mut notes = vec![format!("skill read: {name}")];
    if let Some(note) = note {
        notes.push(note);
    }
    Ok((text, notes))
}

/// Keep the longest prefix that stays inside `cap` and does not end in a partial heading.
pub fn trim_skill(text: &str, cap: usize) -> (String, Option<String>) {
    if char_count(text) <= cap {
        return (text.to_string(), None);
    }
    let headings = heading_spans(text);
    let cut = if let Some((start, _)) = headings.iter().rev().find(|(_, end)| *end <= cap) {
        headings
            .iter()
            .find_map(|(next, _)| (*next > *start && *next <= cap).then_some(*next))
            .unwrap_or(cap)
    } else {
        cap
    };
    (
        prefix_chars(text, cut).to_string(),
        Some(format!(
            "skill cut to {cut} characters at the last heading that fits"
        )),
    )
}

fn heading_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut char_index = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let lead = line.len() - line.trim_start().len();
            let lead_chars = line[..lead].chars().count();
            let start = char_index + lead_chars;
            let visible = line.trim_end_matches(['\n', '\r']);
            let end = char_index + visible.chars().count();
            spans.push((start, end));
        }
        char_index += line.chars().count();
    }
    spans
}

fn sha256(text: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}:{}", text.len())
}
