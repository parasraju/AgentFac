use crate::slices::count_slices;

/// Bug words are only these four. `fix`, `error`, and `fail` stay out on purpose.
const BUG_WORDS: &[&str] = &["bug", "broken", "crash", "regression"];
const NOT_BUG_WORDS: &[&str] = &["fix", "error", "fail"];
const CHANGE_WORDS: &[&str] = &[
    "add",
    "change",
    "update",
    "implement",
    "create",
    "write",
    "refactor",
    "rename",
    "remove",
    "introduce",
    "migrate",
    "support",
    "feature",
    "extend",
    "replace",
];
const VERBS: &[&str] = &[
    "add",
    "fix",
    "update",
    "change",
    "implement",
    "create",
    "write",
    "refactor",
    "rename",
    "remove",
    "delete",
    "move",
    "introduce",
    "migrate",
    "debug",
    "repair",
    "patch",
    "handle",
    "make",
    "build",
    "set",
    "allow",
    "return",
    "skip",
    "run",
    "open",
    "parse",
    "format",
    "trim",
    "pack",
    "test",
    "check",
    "show",
    "hide",
    "send",
    "read",
    "stop",
    "start",
    "call",
    "log",
    "edit",
    "cut",
    "ship",
    "use",
    "put",
    "get",
    "drop",
    "load",
    "save",
    "init",
    "crash",
    "break",
    "fail",
    "sort",
    "filter",
    "map",
    "wrap",
    "hook",
    "revert",
    "undo",
    "cache",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bug,
    Change,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Bug => "bug",
            Kind::Change => "change",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "bug" => Some(Kind::Bug),
            "change" => Some(Kind::Change),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    DiagnosingBugs,
    ToSpec,
    Tdd,
    Implement,
    CodeReview,
    Unslop,
    Pr,
    Architect,
    Other(String),
}

impl Stage {
    pub fn name(&self) -> &str {
        match self {
            Stage::DiagnosingBugs => "diagnosing-bugs",
            Stage::ToSpec => "to-spec",
            Stage::Tdd => "tdd",
            Stage::Implement => "implement",
            Stage::CodeReview => "code-review",
            Stage::Unslop => "unslop",
            Stage::Pr => "pr",
            Stage::Architect => "architect",
            Stage::Other(name) => name,
        }
    }

    pub fn parse(name: &str) -> Self {
        match name {
            "diagnosing-bugs" => Stage::DiagnosingBugs,
            "to-spec" => Stage::ToSpec,
            "tdd" => Stage::Tdd,
            "implement" => Stage::Implement,
            "code-review" => Stage::CodeReview,
            "unslop" => Stage::Unslop,
            "pr" => Stage::Pr,
            "architect" => Stage::Architect,
            other => Stage::Other(other.to_string()),
        }
    }
}

pub fn chain_arrow(chain: &[Stage]) -> String {
    chain
        .iter()
        .map(Stage::name)
        .collect::<Vec<_>>()
        .join(" → ")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecSource {
    None,
    Task,
    File(String),
    Run,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub chain: Vec<Stage>,
    pub kind: Option<Kind>,
    pub spec_from: SpecSource,
    pub branch: String,
    pub test_command: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Stop { message: String },
    Wait { plan: Plan, message: String },
    Ready(Plan),
}

#[derive(Clone, Debug)]
pub struct Facts {
    pub task: String,
    pub kind_flag: Option<Kind>,
    pub yes: bool,
    pub forced_skill: Option<String>,
    pub status_clean: bool,
    pub branch: String,
    pub test_command: String,
    pub named_files: Vec<String>,
    pub named_spec: Option<String>,
    pub run_spec_exists: bool,
    pub spec_slices: usize,
}

impl Facts {
    pub fn new(task: impl Into<String>) -> Self {
        Self {
            task: task.into(),
            kind_flag: None,
            yes: false,
            forced_skill: None,
            status_clean: true,
            branch: String::new(),
            test_command: String::new(),
            named_files: Vec::new(),
            named_spec: None,
            run_spec_exists: false,
            spec_slices: 0,
        }
    }
}

pub fn pick(facts: &Facts) -> Decision {
    let task = facts.task.trim();
    let word_list = words(task);
    if task.is_empty() || (task.chars().count() < 12 && !has_word(&word_list, VERBS)) {
        return Decision::Stop {
            message: "Ask for a task. No skill runs.".into(),
        };
    }
    if task.contains('?') || starts_with_question(task) {
        return Decision::Stop {
            message: "Agentfac ships code. It does not answer questions.".into(),
        };
    }
    if let Some(skill) = facts.forced_skill.as_deref() {
        let name = skill.trim();
        if name.is_empty() {
            return Decision::Stop {
                message: "Ask for a skill name. No skill runs.".into(),
            };
        }
        return Decision::Ready(plan_for(
            facts,
            vec![Stage::parse(name)],
            facts.kind_flag,
            SpecSource::None,
        ));
    }

    let has_bug = has_bug_word(&word_list);
    let has_change = has_word(&word_list, CHANGE_WORDS);
    let kind = match facts.kind_flag {
        Some(kind) => kind,
        None if has_bug => Kind::Bug,
        None => Kind::Change,
    };

    let specified = facts.named_spec.is_some() || facts.run_spec_exists;
    let one_file = kind != Kind::Bug && facts.named_files.len() == 1 && facts.status_clean;
    let spec_from = if let Some(path) = &facts.named_spec {
        SpecSource::File(path.clone())
    } else if facts.run_spec_exists {
        SpecSource::Run
    } else if one_file {
        SpecSource::Task
    } else {
        SpecSource::None
    };

    let slices = if matches!(spec_from, SpecSource::Task) {
        facts.spec_slices.max(count_slices(task))
    } else {
        facts.spec_slices
    };
    let skip_to_spec = kind == Kind::Bug || !matches!(spec_from, SpecSource::None);

    let mut chain = Vec::new();
    if kind == Kind::Bug {
        chain.push(Stage::DiagnosingBugs);
    }
    if !skip_to_spec {
        chain.push(Stage::ToSpec);
    }
    chain.extend([
        Stage::Tdd,
        Stage::Implement,
        Stage::CodeReview,
        Stage::Unslop,
    ]);
    if slices >= 2 {
        chain.push(Stage::Architect);
    } else {
        chain.push(Stage::Pr);
    }

    let plan = plan_for(facts, chain, Some(kind), spec_from);
    let vague = facts.named_files.is_empty() && !specified && !has_bug;
    let ambiguous = facts.kind_flag.is_none() && has_bug && has_change;
    if (vague || ambiguous) && !facts.yes {
        let why = if ambiguous {
            "This task looks like a bug and a change."
        } else {
            "This task names no file and no bug."
        };
        let message = format!(
            "{why} Guessed chain:\n{}\nRe-run with --yes to continue.",
            chain_arrow(&plan.chain)
        );
        return Decision::Wait { plan, message };
    }
    Decision::Ready(plan)
}

pub fn chain_file(plan: &Plan) -> String {
    let mut lines = Vec::new();
    lines.push(format!("branch: {}", display_or_dash(&plan.branch)));
    lines.push(format!("test: {}", display_or_dash(&plan.test_command)));
    if let Some(kind) = plan.kind {
        lines.push(format!("kind: {}", kind.as_str()));
    }
    match &plan.spec_from {
        SpecSource::File(path) => lines.push(format!("spec: {path}")),
        SpecSource::Task => lines.push("spec: task".into()),
        SpecSource::Run => lines.push("spec: run".into()),
        SpecSource::None => {}
    }
    for stage in &plan.chain {
        lines.push(stage.name().to_string());
    }
    lines.push(String::new());
    lines.join("\n")
}

pub fn skills_text() -> String {
    "\
Agentfac picks a chain locally. No model call.

Rules, in order. The first match wins.
1. Empty text, or text under 12 characters with no verb: ask for a task.
2. A question (? or a first word of how, why, or what): stop. Agentfac ships code.
3. --kind bug or --kind change wins over keywords. Bug words: bug, broken, crash, regression. fix, error, and fail do not decide the kind. A clear bug adds diagnosing-bugs and skips to-spec. A bug word that also reads as a change, or no file and no bug word, prints the guessed chain and waits. --yes is the only way past that wait.
4. A path in the text that ends in .md, and exists, is the spec. Skip to-spec.
5. Exactly one existing file and a clean git status: the task is the spec. Skip to-spec.
6. Everything else is a change. The chain adds to-spec.

Then the same tail: tdd, implement, code-review, unslop, and pr. Two or more slices in the spec replace pr with architect. A forced --skill replaces this picker.

Bug
  diagnosing-bugs → tdd → implement → code-review → unslop → pr

Small change
  tdd → implement → code-review → unslop → pr

Larger change
  to-spec → tdd → implement → code-review → unslop → pr

Larger change with two or more slices
  to-spec → tdd → implement → code-review → unslop → architect
"
    .to_string()
}

fn plan_for(facts: &Facts, chain: Vec<Stage>, kind: Option<Kind>, spec_from: SpecSource) -> Plan {
    Plan {
        chain,
        kind,
        spec_from,
        branch: facts.branch.clone(),
        test_command: facts.test_command.clone(),
    }
}

fn display_or_dash(text: &str) -> &str {
    if text.is_empty() {
        "-"
    } else {
        text
    }
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.to_ascii_lowercase())
        .collect()
}

fn has_word(words: &[String], list: &[&str]) -> bool {
    words
        .iter()
        .any(|word| list.iter().any(|item| word == item))
}

fn has_bug_word(words: &[String]) -> bool {
    words.iter().any(|word| {
        BUG_WORDS.iter().any(|item| word == item) && !NOT_BUG_WORDS.iter().any(|item| word == item)
    })
}

fn starts_with_question(task: &str) -> bool {
    let Some(first) = task.split_whitespace().next() else {
        return false;
    };
    let first = first.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    matches!(first.to_ascii_lowercase().as_str(), "how" | "why" | "what")
}
