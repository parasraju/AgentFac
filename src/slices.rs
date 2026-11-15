use std::fs;
use std::path::Path;

use crate::paths::{path_tokens, same_path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceStatus {
    Pending,
    Passed,
    Failed,
    Skipped,
    Draft,
}

impl SliceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SliceStatus::Pending => "pending",
            SliceStatus::Passed => "passed",
            SliceStatus::Failed => "failed",
            SliceStatus::Skipped => "skipped",
            SliceStatus::Draft => "draft",
        }
    }

    pub fn parse(text: &str) -> Self {
        match text.trim() {
            "passed" => SliceStatus::Passed,
            "failed" => SliceStatus::Failed,
            "skipped" => SliceStatus::Skipped,
            "draft" => SliceStatus::Draft,
            _ => SliceStatus::Pending,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    pub name: String,
    pub files: Vec<String>,
    pub detail: String,
    pub status: SliceStatus,
    pub note: String,
}

impl Slice {
    pub fn new(name: impl Into<String>, files: Vec<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            files,
            detail: detail.into(),
            status: SliceStatus::Pending,
            note: String::new(),
        }
    }
}

pub fn count_slices(spec: &str) -> usize {
    if let Some(items) = section_bullets(spec, "slices") {
        return items.len();
    }
    slice_headings(spec).len()
}

pub fn slices_from_spec(spec: &str) -> Vec<Slice> {
    if let Some(items) = section_bullets(spec, "slices") {
        return items
            .into_iter()
            .map(|item| parse_slice_bullet(&item))
            .collect();
    }
    slice_headings(spec)
}

pub fn parse_agent_slices(output: &str) -> Vec<Slice> {
    let mut lines = output.lines();
    while let Some(line) = lines.next() {
        if line.trim() == "AGENTFAC_SLICES:" {
            let mut slices = Vec::new();
            for line in lines.by_ref() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Some(rest) = trimmed.strip_prefix("- ") else {
                    break;
                };
                slices.push(parse_slice_bullet(rest));
            }
            return slices;
        }
    }
    Vec::new()
}

pub fn parse_slice_bullet(text: &str) -> Slice {
    let parts: Vec<&str> = text.split('|').map(str::trim).collect();
    if parts.len() >= 2 {
        let name = parts[0].to_string();
        let mut files = Vec::new();
        let mut detail = String::new();
        for part in &parts[1..] {
            if let Some(rest) = part.strip_prefix("files:") {
                files = rest
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect();
            } else if detail.is_empty() {
                detail = (*part).to_string();
            } else {
                detail.push(' ');
                detail.push_str(part);
            }
        }
        if detail.is_empty() {
            detail = name.clone();
        }
        return Slice::new(name, files, detail);
    }
    let files = path_tokens(text);
    Slice::new(text, files, text)
}

pub fn shares_file(slice: &Slice, blocked: &[String]) -> bool {
    slice
        .files
        .iter()
        .any(|file| blocked.iter().any(|other| same_path(file, other)))
}

pub fn write_slices(path: &Path, slices: &[Slice]) -> std::io::Result<()> {
    let mut out = String::from("# Slices\n");
    for slice in slices {
        out.push_str(&format!(
            "\n## {}\nstatus: {}\nfiles: {}\ndetail: {}\nnote: {}\n",
            one_line(&slice.name),
            slice.status.as_str(),
            slice.files.join(", "),
            one_line(&slice.detail),
            one_line(&slice.note),
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, out)
}

pub fn read_slices(text: &str) -> Vec<Slice> {
    let mut slices = Vec::new();
    let mut current: Option<Slice> = None;
    for line in text.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            if let Some(slice) = current.take() {
                slices.push(slice);
            }
            current = Some(Slice::new(title.trim(), Vec::new(), ""));
            continue;
        }
        let Some(slice) = current.as_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("status:") {
            slice.status = SliceStatus::parse(rest);
        } else if let Some(rest) = line.strip_prefix("files:") {
            slice.files = rest
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect();
        } else if let Some(rest) = line.strip_prefix("detail:") {
            slice.detail = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("note:") {
            slice.note = rest.trim().to_string();
        }
    }
    if let Some(slice) = current {
        slices.push(slice);
    }
    slices
}

fn one_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ").trim().to_string()
}

fn section_bullets(spec: &str, section: &str) -> Option<Vec<String>> {
    let mut found = false;
    let mut level = 0;
    let mut items = Vec::new();
    let mut fence = false;
    for line in spec.lines() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        if let Some((heading_level, title)) = heading(line) {
            if found {
                if heading_level <= level {
                    return Some(items);
                }
                continue;
            }
            if title.eq_ignore_ascii_case(section) {
                found = true;
                level = heading_level;
            }
            continue;
        }
        if !found {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            items.push(trimmed[2..].trim().to_string());
        } else if let Some(rest) = numbered_item(trimmed) {
            items.push(rest);
        }
    }
    if found {
        Some(items)
    } else {
        None
    }
}

fn slice_headings(spec: &str) -> Vec<Slice> {
    let mut out = Vec::new();
    let mut fence = false;
    for line in spec.lines() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        let Some((_, title)) = heading(line) else {
            continue;
        };
        let lower = title.to_ascii_lowercase();
        if lower == "slice" || lower.starts_with("slice ") {
            let files = path_tokens(&title);
            out.push(Slice::new(title.clone(), files, title));
        }
    }
    out
}

fn heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with('#') {
        return None;
    }
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = trimmed[level..].trim();
    if rest.is_empty() {
        return None;
    }
    Some((level, rest.to_string()))
}

fn numbered_item(text: &str) -> Option<String> {
    let (num, rest) = text.split_once('.')?;
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let rest = rest.trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}
