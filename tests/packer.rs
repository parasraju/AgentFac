use agentfac::format::noise_marks;
use agentfac::pack::{
    build, cap_diff, compose_prompt, excluded, extract_symbol, function_span, PackInput,
    SourceFile, FILE_SPAN_CAP, SKILL_CAP, TASK_CAP,
};
use agentfac::picker::Stage;
use agentfac::skill::trim_skill;

fn file(path: &str, contents: &str, span: bool) -> SourceFile {
    SourceFile {
        path: path.into(),
        contents: contents.into(),
        span,
    }
}

#[test]
fn span_includes_the_whole_function_past_forty_lines() {
    let mut body = String::from("fn alpha() {\n");
    for index in 0..60 {
        body.push_str(&format!("    let v{index} = {index};\n"));
    }
    body.push_str("}\nfn beta() {\n    let z = 1;\n}\n");
    let span = function_span(&body, "alpha").expect("alpha");
    assert!(
        span.text.contains("let v59"),
        "the span stopped early:\n{}",
        span.text
    );
    assert!(!span.text.contains("fn beta"));
    assert!(span.text.lines().count() > 40);
}

#[test]
fn braces_inside_strings_and_comments_stay_inside_the_function() {
    let source = r##"
fn alpha() {
    let tricky = "}";
    // }
    /* } */
    let raw = r#"}"#;
    if true {
        let nested = 1;
    }
    let tail = 7;
}
fn beta() {
    let z = 1;
}
"##;
    let span = function_span(source, "alpha").expect("alpha");
    assert!(span.text.contains("let tail = 7"));
    assert!(!span.text.contains("fn beta"));
}

#[test]
fn indent_blocks_pack_the_whole_python_function() {
    let source = "def alpha():\n    value = 1\n    return value\n\ndef beta():\n    return 2\n";
    let span = function_span(source, "alpha").expect("alpha");
    assert!(span.text.contains("return value"));
    assert!(!span.text.contains("def beta"));
}

#[test]
fn a_missing_symbol_stops_before_a_model_call() {
    let err = function_span("fn other() {}\n", "alpha").unwrap_err();
    assert!(matches!(err, agentfac::pack::SpanError::Missing { .. }));

    let mut input = PackInput::new(Stage::Tdd);
    input.symbol = Some("alpha".into());
    input
        .files
        .push(file("src/app.rs", "fn other() {}\n", true));
    let err = build(&input).unwrap_err();
    assert!(err.to_string().contains("missing"));
}

#[test]
fn an_oversized_span_splits_into_two_halves() {
    let mut body = String::from("fn alpha() {\n");
    while body.chars().count() <= FILE_SPAN_CAP + 500 {
        body.push_str("    let value = 1;\n");
    }
    body.push_str("}\n");
    let span = function_span(&body, "alpha").expect("alpha");
    assert!(span.text.chars().count() > FILE_SPAN_CAP);

    let mut input = PackInput::new(Stage::Tdd);
    input.symbol = Some("alpha".into());
    input.files.push(file("src/app.rs", &body, true));
    input.test_command = "cargo test".into();
    let packs = build(&input).expect("split");
    assert_eq!(packs.len(), 2);
    assert!(packs[0]
        .notes
        .iter()
        .any(|note| note.contains("part 1 of 2")));
    assert!(packs[1]
        .notes
        .iter()
        .any(|note| note.contains("part 2 of 2")));
    let merged = format!("{}{}", packs[0].text, packs[1].text);
    assert!(merged.contains("fn alpha"));
    assert!(merged.contains("let value = 1"));
}

#[test]
fn caps_cut_diff_tree_readme_and_context() {
    let mut diff = String::from("diff --git a/src/a.rs b/src/a.rs\n");
    for index in 0..400 {
        diff.push_str(&format!("+line {index} of the diff stays verbose\n"));
    }
    let (cut, notes) = cap_diff(&diff);
    assert!(notes.iter().any(|note| note.contains("diff cut")));
    assert!(cut.contains("The diff was cut."));
    assert!(cut.contains("src/a.rs"));
    assert!(cut.contains("+line 0 "));
    assert!(cut.contains("+line 198 "));
    assert!(!cut.contains("+line 250 "));

    let mut input = PackInput::new(Stage::ToSpec);
    input.tree = (0..250)
        .map(|index| format!("src/file{index}.rs"))
        .collect();
    input.readme = Some(
        (0..150)
            .map(|index| format!("readme line {index}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    input
        .files
        .push(file("src/secret.rs", "UNIQUE_BODY", false));
    let packs = build(&input).expect("to-spec");
    let text = &packs[0].text;
    assert!(text.contains("src/file0.rs"));
    assert!(!text.contains("src/file249.rs"));
    assert!(text.contains("readme line 0"));
    assert!(!text.contains("readme line 149"));
    assert!(!text.contains("UNIQUE_BODY"));
    assert!(packs[0].notes.iter().any(|note| note.contains("tree cut")));
    assert!(packs[0]
        .notes
        .iter()
        .any(|note| note.contains("readme cut")));

    let mut review = PackInput::new(Stage::CodeReview);
    review.spec = Some("# Title only\n\nThe body should stay out.\n".into());
    review.diff = Some("diff --git a/src/a.rs b/src/a.rs\n+ok\n".into());
    review.files.push(file("src/a.rs", "UNIQUE_BODY", true));
    let packs = build(&review).expect("review");
    assert!(packs[0].text.contains("Title only"));
    assert!(!packs[0].text.contains("The body should stay out"));
    assert!(!packs[0].text.contains("UNIQUE_BODY"));
    assert!(packs[0].text.contains("+ok"));

    let mut tiny = review.clone();
    tiny.context_chars = 40;
    tiny.diff = Some("x".repeat(200));
    let packs = build(&tiny).expect("tiny");
    assert!(packs[0]
        .notes
        .iter()
        .any(|note| note.contains("context cut")));
    assert!(packs[0].text.chars().count() <= 40);
}

#[test]
fn a_file_quoted_by_the_spec_is_attached_once() {
    let body = "fn only_once() { let mark = \"UNIQUE_FN_BODY_ZZZ\"; }\n";
    let mut input = PackInput::new(Stage::Tdd);
    input.spec = Some(format!("# Spec\n\n{body}"));
    input.files.push(file("src/app.rs", body, false));
    let packs = build(&input).expect("pack");
    assert_eq!(packs[0].text.matches("UNIQUE_FN_BODY_ZZZ").count(), 1);
    assert!(packs[0]
        .notes
        .iter()
        .any(|note| note.contains("already in spec")));
}

#[test]
fn excluded_files_and_the_task_cap() {
    assert_eq!(
        excluded("node_modules/left-pad/index.js"),
        Some("skipped directory")
    );
    assert_eq!(excluded("Cargo.lock"), Some("lockfile"));
    assert_eq!(excluded("assets/logo.png"), Some("image"));
    let mut input = PackInput::new(Stage::Implement);
    input.files.push(file("Cargo.lock", "lock\n", false));
    assert!(build(&input).is_err());

    let task = "a".repeat(TASK_CAP + 50);
    let (prompt, notes) = compose_prompt("# tdd\n", &task, "context", "", "");
    assert!(notes.iter().any(|note| note.contains("task cut")));
    assert!(!prompt.contains(&"a".repeat(TASK_CAP + 1)));
    assert!(prompt.contains("# tdd"));
    assert!(!prompt.contains("# implement"));
}

#[test]
fn skill_text_cuts_at_the_last_heading_that_fits() {
    let mut text = String::from("# First\n");
    text.push_str(&"a".repeat(100));
    text.push_str("\n## Middle\n");
    text.push_str(&"b".repeat(100));
    text.push_str("\n## Late\n");
    text.push_str(&"c".repeat(500));
    let (cut, note) = trim_skill(&text, 180);
    let note = note.expect("cut");
    assert!(note.contains("heading"));
    assert!(cut.contains("## Middle"));
    assert!(!cut.contains("## Late"));
    assert!(cut.chars().count() <= 180);

    let long = format!("# Head\n{}", "x".repeat(SKILL_CAP + 100));
    let (cut, _) = trim_skill(&long, SKILL_CAP);
    assert!(cut.chars().count() <= SKILL_CAP);
    assert!(cut.starts_with("# Head"));
}

#[test]
fn symbol_extraction_uses_a_definition_keyword() {
    assert_eq!(
        extract_symbol("update src/app.rs fn parse_line today").as_deref(),
        Some("parse_line")
    );
    assert_eq!(extract_symbol("add retry to the client").as_deref(), None);
}

#[test]
fn noise_marks_are_the_unslop_gate() {
    let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
+fn main() {
+    let note = \"wait — here\";
+}
";
    let marks = noise_marks(diff);
    assert!(marks.iter().any(|mark| mark.contains("dash")));
    assert!(noise_marks("+fn main() {\n+    let value = 1;\n+}\n").is_empty());
}
