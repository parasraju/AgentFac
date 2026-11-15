use crate::paths::normalize;
use crate::picker::Stage;
use crate::text::{char_count, last_lines, prefix_chars};

pub const TASK_CAP: usize = 2_000;
pub const CONTEXT_CAP_DEFAULT: usize = 24_000;
pub const SKILL_CAP: usize = 8_000;
pub const FILE_SPAN_CAP: usize = 12_000;
pub const DIFF_CAP: usize = 12_000;
pub const DIFF_LINES: usize = 200;
pub const TREE_CAP: usize = 200;
pub const README_LINES: usize = 100;
pub const TEST_TAIL_LINES: usize = 80;
pub const FILE_BYTE_CAP: u64 = 200 * 1024;

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub path: String,
    pub contents: String,
    /// Pack the function or type that holds the symbol, using brace matching.
    pub span: bool,
}

#[derive(Clone, Debug)]
pub struct PackInput {
    pub stage: Stage,
    pub spec: Option<String>,
    pub symbol: Option<String>,
    pub files: Vec<SourceFile>,
    pub tree: Vec<String>,
    pub readme: Option<String>,
    pub diff: Option<String>,
    pub test_command: String,
    pub failed_test_tail: Option<String>,
    pub review_note: Option<String>,
    pub context_chars: usize,
}

impl PackInput {
    pub fn new(stage: Stage) -> Self {
        Self {
            stage,
            spec: None,
            symbol: None,
            files: Vec::new(),
            tree: Vec::new(),
            readme: None,
            diff: None,
            test_command: String::new(),
            failed_test_tail: None,
            review_note: None,
            context_chars: CONTEXT_CAP_DEFAULT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub part: u8,
    pub parts: u8,
}

#[derive(Clone, Debug)]
pub struct Pack {
    pub text: String,
    pub files: Vec<String>,
    pub notes: Vec<String>,
    pub splices: Vec<Splice>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PackError {
    SymbolMissing { symbol: String, path: String },
    Excluded { path: String, reason: String },
    NeedSymbol { path: String },
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackError::SymbolMissing { symbol, path } => {
                write!(f, "symbol `{symbol}` is missing in {path}")
            }
            PackError::Excluded { path, reason } => write!(f, "{path} is excluded ({reason})"),
            PackError::NeedSymbol { path } => write!(
                f,
                "{path} is over {FILE_SPAN_CAP} characters and names no function or type"
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SpanError {
    Missing { symbol: String },
}

pub fn function_span(source: &str, symbol: &str) -> Result<Span, SpanError> {
    let def = find_def(source, symbol).ok_or_else(|| SpanError::Missing {
        symbol: symbol.to_string(),
    })?;
    let (start, end) = span_bounds(source, def);
    Ok(Span {
        text: source[start..end].to_string(),
        start,
        end,
    })
}

pub fn extract_symbol(text: &str) -> Option<String> {
    const KEYWORDS: &[&str] = &[
        "fn",
        "func",
        "function",
        "def",
        "class",
        "struct",
        "enum",
        "interface",
        "trait",
        "type",
        "method",
    ];
    let words: Vec<&str> = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .collect();
    for index in 1..words.len() {
        if KEYWORDS.contains(&words[index - 1]) && !KEYWORDS.contains(&words[index]) {
            return Some(words[index].to_string());
        }
    }
    None
}

pub fn build(input: &PackInput) -> Result<Vec<Pack>, PackError> {
    let mut notes = Vec::new();
    let prepared = if wants_files(&input.stage) {
        prepare_files(input, &mut notes)?
    } else {
        if !input.files.is_empty() {
            notes.push("left out file bodies".into());
        }
        Vec::new()
    };
    let split = prepared.iter().any(|piece| piece.parts == 2);
    let part_count: u8 = if split { 2 } else { 1 };
    let mut packs = Vec::new();
    for part in 1..=part_count {
        let mut sections: Vec<(String, String)> = Vec::new();
        let mut pack_notes = notes.clone();
        let mut files = Vec::new();
        let mut splices = Vec::new();
        push_spec(input, &mut sections);
        push_diff(input, &mut sections, &mut pack_notes);
        push_tree(input, &mut sections, &mut pack_notes);
        push_readme(input, &mut sections, &mut pack_notes);
        push_test(input, &mut sections);
        if let Some(tail) = input.failed_test_tail.as_deref() {
            if wants_tail(&input.stage) && !tail.is_empty() {
                sections.push(("test failure".into(), last_lines(tail, TEST_TAIL_LINES)));
            }
        }
        if matches!(input.stage, Stage::Implement) {
            if let Some(note) = input.review_note.as_deref() {
                if !note.trim().is_empty() {
                    sections.push(("review note".into(), note.trim().to_string()));
                }
            }
        }
        for piece in prepared
            .iter()
            .filter(|piece| piece.part == part || piece.parts == 1)
        {
            let label = if piece.parts == 2 {
                format!("file: {} (part {} of 2)", piece.path, piece.part)
            } else {
                format!("file: {}", piece.path)
            };
            sections.push((label, piece.text.clone()));
            if !files.iter().any(|path: &String| path == &piece.path) {
                files.push(piece.path.clone());
            }
            if piece.parts == 2 {
                pack_notes.push(format!(
                    "span split: {} part {} of 2",
                    piece.path, piece.part
                ));
                splices.push(Splice {
                    path: piece.path.clone(),
                    start: piece.start,
                    end: piece.end,
                    part: piece.part,
                    parts: piece.parts,
                });
            }
        }
        let (text, cut) = render_sections(&sections, input.context_chars);
        if let Some(note) = cut {
            pack_notes.push(note);
        }
        packs.push(Pack {
            text,
            files,
            notes: pack_notes,
            splices,
        });
    }
    Ok(packs)
}

pub fn cap_diff(diff: &str) -> (String, Vec<String>) {
    if char_count(diff) <= DIFF_CAP {
        return (diff.to_string(), Vec::new());
    }
    let files = diff_files(diff);
    let lines: Vec<&str> = diff.lines().take(DIFF_LINES).collect();
    let file_list = if files.is_empty() {
        "(none)".to_string()
    } else {
        files
            .iter()
            .map(|file| format!("- {file}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let body = format!(
        "The diff was cut.\nChanged files:\n{file_list}\n\n{}",
        lines.join("\n")
    );
    (
        body,
        vec![format!("diff cut ({} characters)", char_count(diff))],
    )
}

pub fn excluded(path: &str) -> Option<&'static str> {
    let normalized = normalize(path);
    if normalized
        .split('/')
        .any(|part| matches!(part, ".git" | "node_modules" | "target" | "dist"))
    {
        return Some("skipped directory");
    }
    let name = normalized.rsplit('/').next().unwrap_or(&normalized);
    const LOCKS: &[&str] = &[
        "Cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lockb",
        "Gemfile.lock",
        "poetry.lock",
        "composer.lock",
        "go.sum",
        "Package.resolved",
    ];
    if LOCKS.contains(&name) {
        return Some("lockfile");
    }
    if let Some((_, ext)) = name.rsplit_once('.') {
        if matches!(
            ext.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" | "svg"
        ) {
            return Some("image");
        }
    }
    None
}

pub fn spec_parts(spec: &str) -> (String, String) {
    let text = spec.trim();
    if text.is_empty() {
        return (String::new(), String::new());
    }
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("");
    let title = first.trim().trim_start_matches('#').trim().to_string();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    (title, body)
}

pub fn compose_prompt(
    skill: &str,
    task: &str,
    context: &str,
    model: &str,
    contract: &str,
) -> (String, Vec<String>) {
    let mut notes = Vec::new();
    let task_text = if char_count(task) > TASK_CAP {
        notes.push(format!("task cut to {TASK_CAP} characters"));
        prefix_chars(task, TASK_CAP).to_string()
    } else {
        task.to_string()
    };
    let mut prompt = String::new();
    if !model.is_empty() {
        prompt.push_str(&format!("Model: {model}\n\n"));
    }
    prompt.push_str(skill.trim_end());
    prompt.push_str("\n\n## task\n");
    prompt.push_str(task_text.trim_end());
    prompt.push_str("\n\n## context\n");
    prompt.push_str(context.trim());
    if !contract.trim().is_empty() {
        prompt.push_str("\n\n## agentfac\n");
        prompt.push_str(contract.trim());
        prompt.push('\n');
    }
    (prompt, notes)
}

pub fn split_in_half(text: &str) -> (String, String) {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return (String::new(), String::new());
    }
    let mid = chars.len() / 2;
    let window = 400;
    let start = mid.saturating_sub(window);
    let end = (mid + window).min(chars.len().saturating_sub(1));
    let mut chosen = mid;
    let mut best = usize::MAX;
    for index in start..=end {
        if chars[index].1 == '\n' {
            let dist = index.abs_diff(mid);
            if dist < best {
                best = dist;
                chosen = index + 1;
            }
        }
    }
    let byte = chars
        .get(chosen)
        .map(|(byte, _)| *byte)
        .unwrap_or(text.len());
    (text[..byte].to_string(), text[byte..].to_string())
}

struct Piece {
    path: String,
    text: String,
    start: usize,
    end: usize,
    part: u8,
    parts: u8,
}

fn prepare_files(input: &PackInput, notes: &mut Vec<String>) -> Result<Vec<Piece>, PackError> {
    let mut pieces = Vec::new();
    let mut seen = Vec::new();
    for file in &input.files {
        let path = normalize(&file.path);
        if seen.iter().any(|item: &String| item == &path) {
            notes.push(format!("duplicate file left out: {path}"));
            continue;
        }
        seen.push(path.clone());
        if let Some(reason) = excluded(&path) {
            return Err(PackError::Excluded {
                path,
                reason: reason.to_string(),
            });
        }
        if file.contents.len() as u64 > FILE_BYTE_CAP {
            return Err(PackError::Excluded {
                path,
                reason: "over 200KB".into(),
            });
        }
        if let Some(spec) = input.spec.as_deref() {
            if !file.contents.is_empty() && spec.contains(&file.contents) {
                notes.push(format!("file already in spec: {path}"));
                continue;
            }
        }
        pieces.extend(pieces_for(file, input.symbol.as_deref(), notes)?);
    }
    Ok(pieces)
}

fn pieces_for(
    file: &SourceFile,
    symbol: Option<&str>,
    notes: &mut Vec<String>,
) -> Result<Vec<Piece>, PackError> {
    let path = normalize(&file.path);
    let (text, start, end) = if file.span {
        if let Some(symbol) = symbol {
            let span =
                function_span(&file.contents, symbol).map_err(|_| PackError::SymbolMissing {
                    symbol: symbol.to_string(),
                    path: path.clone(),
                })?;
            if span.text != file.contents {
                notes.push(format!("span packed: {path} {symbol}"));
            }
            (span.text, span.start, span.end)
        } else if char_count(&file.contents) > FILE_SPAN_CAP {
            return Err(PackError::NeedSymbol { path });
        } else {
            (file.contents.clone(), 0, file.contents.len())
        }
    } else {
        (file.contents.clone(), 0, file.contents.len())
    };
    if char_count(&text) > FILE_SPAN_CAP {
        let (top, bottom) = split_in_half(&text);
        Ok(vec![
            Piece {
                path: path.clone(),
                text: top,
                start,
                end,
                part: 1,
                parts: 2,
            },
            Piece {
                path,
                text: bottom,
                start,
                end,
                part: 2,
                parts: 2,
            },
        ])
    } else {
        Ok(vec![Piece {
            path,
            text,
            start,
            end,
            part: 1,
            parts: 1,
        }])
    }
}

fn push_spec(input: &PackInput, sections: &mut Vec<(String, String)>) {
    let Some(spec) = input.spec.as_deref() else {
        return;
    };
    if spec.trim().is_empty() {
        return;
    }
    if wants_title_only(&input.stage) {
        let (title, _) = spec_parts(spec);
        if !title.is_empty() {
            sections.push(("spec title".into(), title));
        }
        return;
    }
    if wants_spec_body(&input.stage) {
        sections.push(("spec".into(), spec.trim().to_string()));
    }
}

fn push_diff(input: &PackInput, sections: &mut Vec<(String, String)>, notes: &mut Vec<String>) {
    if !wants_diff(&input.stage) {
        return;
    }
    let diff = input.diff.as_deref().unwrap_or("");
    if diff.is_empty() {
        sections.push(("diff".into(), "(empty)".into()));
        notes.push("diff empty".into());
        return;
    }
    let (body, diff_notes) = cap_diff(diff);
    notes.extend(diff_notes);
    sections.push(("diff".into(), body));
}

fn push_tree(input: &PackInput, sections: &mut Vec<(String, String)>, notes: &mut Vec<String>) {
    if !matches!(input.stage, Stage::ToSpec) {
        return;
    }
    let kept: Vec<String> = if input.tree.len() > TREE_CAP {
        notes.push(format!("tree cut at {TREE_CAP} paths"));
        input.tree[..TREE_CAP].to_vec()
    } else {
        input.tree.clone()
    };
    let body = if kept.is_empty() {
        "(empty)".to_string()
    } else {
        kept.join("\n")
    };
    sections.push(("tree".into(), body));
}

fn push_readme(input: &PackInput, sections: &mut Vec<(String, String)>, notes: &mut Vec<String>) {
    if !matches!(input.stage, Stage::ToSpec) {
        return;
    }
    let Some(readme) = input.readme.as_deref() else {
        return;
    };
    let lines: Vec<&str> = readme.lines().collect();
    if lines.len() > README_LINES {
        notes.push(format!("readme cut at {README_LINES} lines"));
        sections.push(("readme".into(), lines[..README_LINES].join("\n")));
    } else {
        sections.push(("readme".into(), readme.to_string()));
    }
}

fn push_test(input: &PackInput, sections: &mut Vec<(String, String)>) {
    if !matches!(input.stage, Stage::Tdd) {
        return;
    }
    let name = if input.test_command.is_empty() {
        "(none)".to_string()
    } else {
        input.test_command.clone()
    };
    sections.push(("test".into(), name));
}

fn render_sections(sections: &[(String, String)], cap: usize) -> (String, Option<String>) {
    let mut text = String::new();
    for (title, body) in sections {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("## ");
        text.push_str(title);
        text.push('\n');
        text.push_str(body);
        text.push('\n');
    }
    if char_count(&text) > cap {
        (
            prefix_chars(&text, cap).to_string(),
            Some(format!("context cut to {cap} characters")),
        )
    } else {
        (text, None)
    }
}

fn wants_files(stage: &Stage) -> bool {
    matches!(
        stage,
        Stage::DiagnosingBugs | Stage::Tdd | Stage::Implement | Stage::Other(_)
    )
}

fn wants_diff(stage: &Stage) -> bool {
    matches!(stage, Stage::CodeReview | Stage::Unslop)
}

fn wants_spec_body(stage: &Stage) -> bool {
    matches!(
        stage,
        Stage::Tdd | Stage::Implement | Stage::Architect | Stage::Pr | Stage::Other(_)
    )
}

fn wants_title_only(stage: &Stage) -> bool {
    matches!(stage, Stage::CodeReview | Stage::Unslop)
}

fn wants_tail(stage: &Stage) -> bool {
    matches!(stage, Stage::DiagnosingBugs | Stage::Tdd | Stage::Implement)
}

fn diff_files(diff: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in diff.lines() {
        let Some(rest) = line.strip_prefix("diff --git ") else {
            continue;
        };
        let Some((_, path)) = rest.split_once(" b/") else {
            continue;
        };
        if !files.iter().any(|item: &String| item == path) {
            files.push(path.to_string());
        }
    }
    files
}

fn find_def(source: &str, symbol: &str) -> Option<usize> {
    const KEYWORDS: &[&str] = &[
        "fn",
        "func",
        "function",
        "def",
        "class",
        "struct",
        "enum",
        "interface",
        "trait",
        "type",
        "impl",
        "method",
        "const",
    ];
    let mut offset = 0usize;
    for line in source.split_inclusive('\n') {
        if line_defines(line, symbol, KEYWORDS) {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn line_defines(line: &str, symbol: &str, keywords: &[&str]) -> bool {
    let code = line.split("//").next().unwrap_or(line);
    let words: Vec<&str> = code
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .collect();
    for index in 1..words.len() {
        if words[index] == symbol && keywords.contains(&words[index - 1]) {
            return true;
        }
    }
    false
}

fn span_bounds(source: &str, def_start: usize) -> (usize, usize) {
    if let Some(end) = brace_end(source, def_start) {
        (def_start, end)
    } else {
        indent_end(source, def_start)
    }
}

fn brace_end(source: &str, def_start: usize) -> Option<usize> {
    let chars: Vec<(usize, char)> = source.char_indices().collect();
    let mut state = 0i32;
    let mut escape = false;
    let mut raw_hashes = 0usize;
    let mut depth = 0i32;
    let mut started = false;
    let mut index = 0usize;
    while index < chars.len() {
        let (byte, ch) = chars[index];
        let next = chars.get(index + 1).map(|item| item.1);
        match state {
            1 => {
                if ch == '\n' {
                    state = 0;
                }
            }
            2 => {
                if ch == '*' && next == Some('/') {
                    state = 0;
                    index += 1;
                }
            }
            3 => {
                if escape {
                    escape = false;
                } else if ch == '\\' {
                    escape = true;
                } else if ch == '"' {
                    state = 0;
                }
            }
            4 => {
                if escape {
                    escape = false;
                } else if ch == '\\' {
                    escape = true;
                } else if ch == '\'' {
                    state = 0;
                }
            }
            5 => {
                if ch == '"' {
                    let mut hashes = 0usize;
                    let mut look = index + 1;
                    while look < chars.len() && chars[look].1 == '#' && hashes < raw_hashes {
                        hashes += 1;
                        look += 1;
                    }
                    if hashes == raw_hashes {
                        state = 0;
                        index = look - 1;
                    }
                }
            }
            _ => {
                if ch == '/' && next == Some('/') {
                    state = 1;
                    index += 1;
                } else if ch == '/' && next == Some('*') {
                    state = 2;
                    index += 1;
                } else if ch == '"' {
                    state = 3;
                } else if ch == '\'' {
                    state = 4;
                } else if let Some(raw) = raw_string_open(&chars, index) {
                    state = 5;
                    raw_hashes = raw.0;
                    index = raw.1;
                } else if byte >= def_start && ch == '{' {
                    depth += 1;
                    started = true;
                } else if started && ch == '}' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(byte + ch.len_utf8());
                    }
                }
            }
        }
        index += 1;
    }
    None
}

fn raw_string_open(chars: &[(usize, char)], index: usize) -> Option<(usize, usize)> {
    if chars.get(index).map(|item| item.1) != Some('r') {
        return None;
    }
    let mut look = index + 1;
    let mut hashes = 0usize;
    while look < chars.len() && chars[look].1 == '#' {
        hashes += 1;
        look += 1;
    }
    if look < chars.len() && chars[look].1 == '"' {
        Some((hashes, look))
    } else {
        None
    }
}

fn indent_end(source: &str, def_start: usize) -> (usize, usize) {
    let rest = &source[def_start..];
    let base = indent_width(rest.lines().next().unwrap_or(""));
    let mut end = def_start;
    let mut first = true;
    for line in rest.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        if !first && !body.trim().is_empty() && indent_width(body) <= base {
            break;
        }
        first = false;
        end += line.len();
    }
    (def_start, end)
}

fn indent_width(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}
