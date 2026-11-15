use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentfac")
}

struct Repo {
    dir: PathBuf,
    home: PathBuf,
    base: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let base =
            std::env::temp_dir().join(format!("agentfac-{}-{}", std::process::id(), unique()));
        let dir = base.join("repo");
        let home = base.join("home");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&home).unwrap();
        Self { dir, home, base }
    }

    fn git_init(&self) {
        fs::create_dir_all(&self.dir).unwrap();
        run(self, &["git", "init", "-b", "main"]);
        run(self, &["git", "config", "user.email", "test@example.com"]);
        run(self, &["git", "config", "user.name", "Test"]);
    }

    fn write(&self, path: &str, text: &str) {
        let full = self.dir.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(full, text).unwrap();
    }

    fn commit_all(&self, message: &str) {
        run(self, &["git", "add", "-A"]);
        run(self, &["git", "commit", "-m", message]);
    }

    fn agentfac(&self, args: &[&str]) -> std::process::Output {
        Command::new(bin())
            .args(args)
            .current_dir(&self.dir)
            .env("AGENTFAC_HOME", &self.home)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap()
    }
}

fn unique() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

fn run(repo: &Repo, args: &[&str]) {
    let status = Command::new(args[0])
        .args(&args[1..])
        .current_dir(&repo.dir)
        .status()
        .unwrap();
    assert!(status.success(), "{args:?} failed");
}

fn skills(repo: &Repo) {
    for (name, body) in [
        ("tdd", "# tdd\nWrite one test.\n"),
        ("implement", "# implement\nImplement the change.\n"),
        ("code-review", "# code-review\nReview the diff.\n"),
        ("unslop", "# unslop\nRemove noise.\n"),
        ("pr", "# pr\nOpen a pull request.\n"),
        ("to-spec", "# to-spec\nWrite a spec.\n"),
        (
            "diagnosing-bugs",
            "# diagnosing-bugs\nDiagnose the failure.\n",
        ),
        ("architect", "# architect\nList the slices.\n"),
    ] {
        repo.write(&format!(".agentfac/skills/{name}.md"), body);
    }
}

fn agent_script(fail_review: bool) -> String {
    let review = if fail_review { "fail" } else { "pass" };
    format!(
        r#"#!/bin/sh
log="$1"
input=$(cat)
printf '%s\n---PROMPT---\n' "$input" >> "$log"
if printf '%s' "$input" | grep -q '^# code-review'; then
  echo {review}
  exit 0
fi
if printf '%s' "$input" | grep -q FAILSLICE; then
  echo 'agent failed the slice' >&2
  exit 1
fi
if printf '%s' "$input" | grep -q '^# implement'; then
  mkdir -p src
  printf 'fn main() {{\n    let _note = "started";\n}}\n' > src/app.rs
  echo 'AGENTFAC_PATHS: src/app.rs'
  exit 0
fi
if printf '%s' "$input" | grep -q '^# architect'; then
  printf '%s\n' 'AGENTFAC_SLICES:' '- client | files: src/a.rs | FAILSLICE a' '- overlap | files: src/a.rs | keep going' '- other | files: src/c.rs | ok c'
  exit 0
fi
echo 'AGENTFAC_PATHS: src/app.rs'
exit 0
"#
    )
}

fn prepare_app(repo: &Repo, agent: &Path, log: &Path, review_fail: bool) {
    repo.git_init();
    skills(repo);
    repo.write("src/app.rs", "fn main() {}\n");
    let script = repo.base.join("agent.sh");
    fs::write(&script, agent_script(review_fail)).unwrap();
    let gh = repo.base.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\nprintf '%s\n' \"$*\" >> \"$GH_LOG\"\necho https://example.com/pull/1\n",
    )
    .unwrap();
    let _ = std::process::Command::new("chmod")
        .args(["+x", script.to_str().unwrap(), gh.to_str().unwrap()])
        .status();
    repo.write(
        ".agentfac/config.toml",
        &format!(
            "agent = \"sh {} {}\"\ntest = \"true\"\ncontext_chars = 24000\nmodel = \"\"\nunslop_model = \"\"\nformatter = \"\"\nskills_dir = \"\"\n",
            script.display(),
            log.display()
        ),
    );
    repo.commit_all("start");
    let bare = repo.base.join("bare.git");
    let status = Command::new("git")
        .args(["init", "--bare", bare.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    run(
        repo,
        &["git", "remote", "add", "origin", bare.to_str().unwrap()],
    );
    let _ = agent;
}

fn output_text(output: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn dry_run_prints_the_chain_without_calling_the_agent() {
    let repo = Repo::new();
    let log = repo.base.join("agent.log");
    prepare_app(&repo, &repo.base.join("agent.sh"), &log, false);
    let output = repo.agentfac(&["run", "--dry", "--yes", "add retry to the client please"]);
    let text = output_text(&output);
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(
        text.contains("to-spec → tdd → implement → code-review → unslop → pr"),
        "{text}"
    );
    assert!(!log.exists());
}

#[test]
fn a_vague_task_waits_and_does_not_call_the_model() {
    let repo = Repo::new();
    let log = repo.base.join("agent.log");
    prepare_app(&repo, &repo.base.join("agent.sh"), &log, false);
    let output = repo.agentfac(&["run", "add retry to the client please"]);
    let text = output_text(&output);
    assert_eq!(output.status.code(), Some(2), "{text}");
    assert!(text.contains("--yes"), "{text}");
    assert!(!log.exists());
}

#[test]
fn questions_and_a_missing_agent_stop_early() {
    let repo = Repo::new();
    repo.git_init();
    repo.write("README.md", "hi\n");
    repo.commit_all("start");
    let output = repo.agentfac(&["run", "how does retry work in this repo"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output_text(&output).contains("does not answer questions"));

    repo.write(
        ".agentfac/config.toml",
        "agent = \"\"\ntest = \"\"\ncontext_chars = 24000\nmodel = \"\"\nunslop_model = \"\"\nformatter = \"\"\nskills_dir = \"\"\n",
    );
    repo.write("src/app.rs", "fn main() {}\n");
    repo.commit_all("app");
    let output = repo.agentfac(&["run", "update src/app.rs to log one line"]);
    let text = output_text(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("agent command is empty"), "{text}");
}

#[test]
fn a_missing_symbol_stops_before_the_agent() {
    let repo = Repo::new();
    let log = repo.base.join("agent.log");
    prepare_app(&repo, &repo.base.join("agent.sh"), &log, false);
    let output = repo.agentfac(&["run", "update src/app.rs fn missing_symbol today"]);
    let text = output_text(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("missing"), "{text}");
    assert!(
        !log.exists(),
        "{}",
        fs::read_to_string(&log).unwrap_or_default()
    );
}

#[test]
fn one_review_pass_opens_a_pull_request_and_skips_unslop() {
    let repo = Repo::new();
    let log = repo.base.join("agent.log");
    let gh_log = repo.base.join("gh.log");
    prepare_app(&repo, &repo.base.join("agent.sh"), &log, false);
    let task = "# Add a log line\n\nWrite a log line in src/app.rs when the process starts.\n";
    let output = Command::new(bin())
        .arg("run")
        .arg(task)
        .current_dir(&repo.dir)
        .env("AGENTFAC_HOME", &repo.home)
        .env("AGENTFAC_GH", repo.base.join("gh"))
        .env("GH_LOG", &gh_log)
        .output()
        .unwrap();
    let text = output_text(&output);
    assert_eq!(output.status.code(), Some(0), "{text}");
    let prompts = fs::read_to_string(&log).unwrap();
    assert!(prompts.contains("# tdd\n"), "{prompts}");
    assert!(prompts.contains("# implement\n"), "{prompts}");
    assert!(prompts.contains("# code-review\n"), "{prompts}");
    assert!(!prompts.contains("# unslop\n"), "{prompts}");
    assert!(!prompts.contains("# to-spec\n"), "{prompts}");
    let gh = fs::read_to_string(&gh_log).unwrap();
    assert!(gh.contains("pr create"), "{gh}");
    assert!(!gh.contains("--draft"), "{gh}");
    assert!(text.contains("https://example.com/pull/1"), "{text}");
}

#[test]
fn a_second_review_failure_opens_a_draft() {
    let repo = Repo::new();
    let log = repo.base.join("agent.log");
    let gh_log = repo.base.join("gh.log");
    prepare_app(&repo, &repo.base.join("agent.sh"), &log, true);
    let task = "# Add a log line\n\nWrite a log line in src/app.rs when the process starts.\n";
    let output = Command::new(bin())
        .arg("run")
        .arg(task)
        .current_dir(&repo.dir)
        .env("AGENTFAC_HOME", &repo.home)
        .env("AGENTFAC_GH", repo.base.join("gh"))
        .env("GH_LOG", &gh_log)
        .output()
        .unwrap();
    let text = output_text(&output);
    assert_eq!(output.status.code(), Some(0), "{text}");
    let prompts = fs::read_to_string(&log).unwrap();
    assert_eq!(prompts.matches("# code-review\n").count(), 2, "{prompts}");
    assert_eq!(prompts.matches("# implement\n").count(), 2, "{prompts}");
    let gh = fs::read_to_string(&gh_log).unwrap();
    assert!(gh.contains("--draft"), "{gh}");
    assert_eq!(gh.matches("pr create").count(), 1, "{gh}");
}

#[test]
fn init_writes_config_and_a_second_repo_needs_a_key() {
    let home = std::env::temp_dir().join(format!("agentfac-home-{}", unique()));
    fs::create_dir_all(&home).unwrap();
    let first = std::env::temp_dir().join(format!("agentfac-first-{}", unique()));
    let second = std::env::temp_dir().join(format!("agentfac-second-{}", unique()));
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    let run = |dir: &Path, args: &[&str]| {
        Command::new(bin())
            .args(args)
            .current_dir(dir)
            .env("AGENTFAC_HOME", &home)
            .output()
            .unwrap()
    };
    let output = run(&first, &["init", "--agent", "echo hi"]);
    assert_eq!(output.status.code(), Some(0), "{}", output_text(&output));
    assert!(first.join(".git").is_dir());
    let config = fs::read_to_string(first.join(".agentfac/config.toml")).unwrap();
    assert!(config.contains("echo hi"), "{config}");

    let denied = run(&second, &["init", "--agent", "echo hi"]);
    assert_eq!(denied.status.code(), Some(1), "{}", output_text(&denied));
    assert!(output_text(&denied).contains("license"));

    let licensed = Command::new(bin())
        .arg("license")
        .env("AGENTFAC_HOME", &home)
        .output()
        .unwrap();
    assert_eq!(
        licensed.status.code(),
        Some(0),
        "{}",
        output_text(&licensed)
    );
    let key = fs::read_to_string(home.join("license.key")).unwrap();
    assert!(agentfac::license::validate_key(&key));

    let allowed = run(&second, &["init", "--agent", "echo hi"]);
    assert_eq!(allowed.status.code(), Some(0), "{}", output_text(&allowed));
    assert!(second.join(".git").is_dir());
}

#[test]
fn resume_reruns_failed_slices_and_skips_file_overlap() {
    let repo = Repo::new();
    let log = repo.base.join("agent.log");
    prepare_app(&repo, &repo.base.join("agent.sh"), &log, false);
    repo.write("src/a.rs", "fn a() {}\n");
    repo.write("src/c.rs", "fn c() {}\n");
    repo.write(
        ".agentfac/runs/demo/slices.md",
        "\
# Slices

## client
status: failed
files: src/a.rs
detail: FAILSLICE a
note: test failed

## overlap
status: failed
files: src/a.rs
detail: ok overlap
note: 

## other
status: failed
files: src/c.rs
detail: ok c
note: 
",
    );
    repo.write(".agentfac/runs/demo/spec.md", "# Work\n\nDo the slices.\n");
    repo.write(".agentfac/runs/demo/task.txt", "do the slices\n");
    let output = repo.agentfac(&["resume", "demo"]);
    let text = output_text(&output);
    assert_eq!(output.status.code(), Some(0), "{text}");
    let slices = fs::read_to_string(repo.dir.join(".agentfac/runs/demo/slices.md")).unwrap();
    assert!(slices.contains("## client\nstatus: failed"), "{slices}");
    assert!(slices.contains("## overlap\nstatus: skipped"), "{slices}");
    assert!(slices.contains("## other\nstatus: passed"), "{slices}");
}
