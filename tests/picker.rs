use agentfac::picker::{pick, skills_text, Decision, Facts, Kind, Stage};
use agentfac::slices::count_slices;

fn facts(task: &str) -> Facts {
    Facts::new(task)
}

fn ready(task: &str) -> agentfac::picker::Plan {
    match pick(&facts(task)) {
        Decision::Ready(plan) => plan,
        other => panic!("expected a chain, got {other:?}"),
    }
}

fn names(task: &str) -> Vec<String> {
    ready(task)
        .chain
        .iter()
        .map(|stage| stage.name().to_string())
        .collect()
}

fn tail(ship: &str) -> Vec<String> {
    ["tdd", "implement", "code-review", "unslop", ship]
        .into_iter()
        .map(str::to_string)
        .collect()
}

#[test]
fn empty_and_short_text_ask_for_a_task() {
    for task in ["", "   ", "hi", "broken"] {
        match pick(&facts(task)) {
            Decision::Stop { message } => assert!(message.contains("Ask for a task"), "{message}"),
            other => panic!("{task:?} should stop, got {other:?}"),
        }
    }
}

#[test]
fn a_short_task_with_a_verb_can_run() {
    let plan = ready("fix crash");
    assert_eq!(plan.chain[0], Stage::DiagnosingBugs);
    assert!(!plan.chain.iter().any(|stage| stage == &Stage::ToSpec));
}

#[test]
fn questions_stop_before_any_chain() {
    for task in [
        "how does retry work here",
        "why is the build slow today",
        "what changed in the client",
        "add retry to the client?",
    ] {
        match pick(&facts(task)) {
            Decision::Stop { message } => assert!(
                message.contains("does not answer questions"),
                "{task}: {message}"
            ),
            other => panic!("{task} should stop, got {other:?}"),
        }
    }
    match pick(&facts("whatever you want added today")) {
        Decision::Wait { .. } | Decision::Ready(_) => {}
        Decision::Stop { message } => panic!("whatever is not a question: {message}"),
    }
}

#[test]
fn bug_words_skip_to_spec_and_fix_does_not() {
    assert_eq!(
        names("the parser crash needs a look today"),
        std::iter::once("diagnosing-bugs".to_string())
            .chain(tail("pr"))
            .collect::<Vec<_>>()
    );
    let mut change = facts("fix the login error in src/client.ts today");
    change.named_files = vec!["src/client.ts".into()];
    change.status_clean = true;
    match pick(&change) {
        Decision::Ready(plan) => {
            assert!(plan
                .chain
                .iter()
                .all(|stage| stage != &Stage::DiagnosingBugs));
            assert!(plan.chain.iter().all(|stage| stage != &Stage::ToSpec));
            assert_eq!(plan.chain.last(), Some(&Stage::Pr));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn vague_and_mixed_tasks_wait_until_yes() {
    match pick(&facts("add retry to the client please")) {
        Decision::Wait { plan, message } => {
            assert!(message.contains("--yes"));
            assert_eq!(plan.chain[0], Stage::ToSpec);
            assert_eq!(plan.kind, Some(Kind::Change));
        }
        other => panic!("{other:?}"),
    }
    let mut yes = facts("add retry to the client please");
    yes.yes = true;
    match pick(&yes) {
        Decision::Ready(plan) => assert_eq!(plan.chain[0], Stage::ToSpec),
        other => panic!("{other:?}"),
    }

    match pick(&facts("add a fallback for the crash today")) {
        Decision::Wait { plan, message } => {
            assert!(message.contains("bug and a change"));
            assert_eq!(plan.chain[0], Stage::DiagnosingBugs);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn kind_flag_wins_and_vague_still_waits() {
    let mut flagged = facts("add a fallback for the crash today");
    flagged.kind_flag = Some(Kind::Change);
    match pick(&flagged) {
        Decision::Ready(plan) => {
            assert_eq!(plan.kind, Some(Kind::Change));
            assert_eq!(plan.chain[0], Stage::ToSpec);
        }
        other => panic!("--kind resolves the mix, got {other:?}"),
    }

    let mut vague = facts("add retry to the client please");
    vague.kind_flag = Some(Kind::Bug);
    match pick(&vague) {
        Decision::Wait { plan, .. } => assert_eq!(plan.chain[0], Stage::DiagnosingBugs),
        other => panic!("a vague task still waits, got {other:?}"),
    }
}

#[test]
fn one_existing_file_on_a_clean_tree_skips_to_spec() {
    let mut one = facts("update src/client.ts to log the retry count");
    one.named_files = vec!["src/client.ts".into()];
    one.status_clean = true;
    match pick(&one) {
        Decision::Ready(plan) => {
            assert!(plan.chain.iter().all(|stage| stage != &Stage::ToSpec));
            assert_eq!(plan.spec_from, agentfac::picker::SpecSource::Task);
        }
        other => panic!("{other:?}"),
    }

    let mut dirty = one.clone();
    dirty.status_clean = false;
    match pick(&dirty) {
        Decision::Ready(plan) => assert_eq!(plan.chain[0], Stage::ToSpec),
        other => panic!("{other:?}"),
    }

    let mut two = facts("update src/a.rs and src/b.rs to log more detail");
    two.named_files = vec!["src/a.rs".into(), "src/b.rs".into()];
    match pick(&two) {
        Decision::Ready(plan) => assert_eq!(plan.chain[0], Stage::ToSpec),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_existing_spec_skips_to_spec_and_two_slices_use_architect() {
    let mut spec = facts("ship the work in docs/plan.md today");
    spec.named_files = vec!["docs/plan.md".into()];
    spec.named_spec = Some("docs/plan.md".into());
    spec.spec_slices = 2;
    match pick(&spec) {
        Decision::Ready(plan) => {
            assert!(plan.chain.iter().all(|stage| stage != &Stage::ToSpec));
            assert_eq!(plan.chain.last(), Some(&Stage::Architect));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_forced_skill_replaces_the_picker() {
    let mut forced = facts("add retry to the client please");
    forced.forced_skill = Some("grill".into());
    match pick(&forced) {
        Decision::Ready(plan) => assert_eq!(plan.chain, vec![Stage::Other("grill".into())]),
        other => panic!("{other:?}"),
    }
    let mut question = facts("how does this work today");
    question.forced_skill = Some("tdd".into());
    assert!(matches!(pick(&question), Decision::Stop { .. }));
}

#[test]
fn skills_text_lists_the_four_chains() {
    let text = skills_text();
    assert!(text.contains("diagnosing-bugs → tdd → implement → code-review → unslop → pr"));
    assert!(text.contains("to-spec → tdd → implement → code-review → unslop → architect"));
    assert!(text.contains("fix, error, and fail do not decide the kind"));
}

#[test]
fn slice_headings_and_fenced_examples() {
    let spec = "\
# Add retry

## Slices

- Client retry in src/client.ts
- Server budget in src/server.ts

```
## Slices
- not a real slice
```
";
    assert_eq!(count_slices(spec), 2);
    let headings = "## Slice 1: client\n\n## Slice 2: server\n";
    assert_eq!(count_slices(headings), 2);
    assert_eq!(count_slices("# Just a title\n\nOne body paragraph.\n"), 0);
}
