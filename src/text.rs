pub fn prefix_chars(text: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    match text.char_indices().nth(n) {
        Some((byte, _)) => &text[..byte],
        None => text,
    }
}

pub fn char_count(text: &str) -> usize {
    text.chars().count()
}

pub fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= n {
        return text.trim_end().to_string();
    }
    lines[lines.len() - n..].join("\n")
}
