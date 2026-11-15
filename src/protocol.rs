pub fn parse_verdict(output: &str) -> Option<bool> {
    let mut found = None;
    for line in output.lines() {
        let mut text = line.trim();
        if let Some(rest) = text.strip_prefix("AGENTFAC_VERDICT:") {
            text = rest.trim();
        }
        if let Some(rest) = text.strip_prefix("verdict:") {
            text = rest.trim();
        }
        if text.eq_ignore_ascii_case("pass") {
            found = Some(true);
        } else if text.eq_ignore_ascii_case("fail") {
            found = Some(false);
        }
    }
    found
}

pub fn parse_paths(output: &str) -> Option<Vec<String>> {
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("AGENTFAC_PATHS:") {
            let paths = rest
                .split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect();
            return Some(paths);
        }
    }
    None
}

pub fn extract_edit(output: &str) -> Option<String> {
    let start = output.find("AGENTFAC_EDIT_START")?;
    let after = &output[start + "AGENTFAC_EDIT_START".len()..];
    let after = after.trim_start_matches(['\r', '\n']);
    let end = after.find("AGENTFAC_EDIT_END")?;
    Some(after[..end].trim_end_matches(['\r', '\n']).to_string())
}

pub fn parse_pr_text(output: &str) -> (Option<String>, Option<String>) {
    let mut title = None;
    let mut body = None;
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("AGENTFAC_TITLE:") {
            title = Some(rest.trim().to_string());
        }
    }
    if let Some(index) = output.find("AGENTFAC_BODY:") {
        let rest = output[index + "AGENTFAC_BODY:".len()..].trim();
        let end = rest.find("\nAGENTFAC_").unwrap_or(rest.len());
        let text = rest[..end].trim();
        if !text.is_empty() {
            body = Some(text.to_string());
        }
    }
    (title, body)
}
