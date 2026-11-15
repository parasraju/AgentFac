/// Path-like tokens in task or spec text. Existence is checked by the caller.
pub fn path_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split_whitespace() {
        let token = clean_token(raw);
        if token.is_empty() || token.contains("://") || token.split('/').any(|p| p == "..") {
            continue;
        }
        if !is_path_like(&token) {
            continue;
        }
        if !out.iter().any(|existing: &String| existing == &token) {
            out.push(token);
        }
    }
    out
}

fn clean_token(raw: &str) -> String {
    let mut token = raw
        .trim_matches(|c: char| "()[]{}<>\"'`,".contains(c))
        .to_string();
    while token.ends_with(['.', ';', ':', '!', '?']) {
        token.pop();
    }
    if let Some((path, line)) = token.rsplit_once(':') {
        if !line.is_empty() && line.chars().all(|c| c.is_ascii_digit()) {
            token = path.to_string();
        }
    }
    token
}

fn is_path_like(token: &str) -> bool {
    if token.contains('/') {
        return true;
    }
    match token.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 8 => {
            ext.chars().all(|c| c.is_ascii_alphabetic())
        }
        _ => false,
    }
}

pub fn same_path(a: &str, b: &str) -> bool {
    normalize(a) == normalize(b)
}

pub fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}
