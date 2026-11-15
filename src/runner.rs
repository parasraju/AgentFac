use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{config_path, Config};
use crate::format::{format_repo, noise_marks};
use crate::gitutil::{self, file_tree, read_readme};
use crate::license::{self, home_dir};
use crate::pack::{self, compose_prompt, spec_parts, PackInput, SourceFile, FILE_BYTE_CAP};
use crate::paths::path_tokens;
use crate::picker::{
    self, chain_arrow, chain_file, Decision, Facts, Kind, Plan, SpecSource, Stage,
};
use crate::protocol::{extract_edit, parse_paths, parse_pr_text, parse_verdict};
use crate::skill::{self, find_skill};
use crate::slices::{self, shares_file, Slice, SliceStatus};
use crate::text::last_lines;

#[derive(Debug)]
pub struct RunError {
    pub message: String,
}

impl RunError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[derive(Clone, Debug)]
pub struct RunOpts {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub task: String,
    pub dry: bool,
    pub yes: bool,
    pub kind: Option<Kind>,
    pub skill: Option<String>,
    pub id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InitOpts {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub agent: Option<String>,
    pub test: Option<String>,
    pub skills_dir: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ResumeOpts {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReviewOutcome {
    Pass,
    Draft,
}

struct Log {
    path: PathBuf,
}

impl Log {
    fn line(&self, text: &str) {
        println!("{text}");
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{text}");
        }
    }
}

struct Session {
    root: PathBuf,
    id: String,
    run_dir: PathBuf,
    cfg: Config,
    task: String,
    spec: Option<String>,
    kind: Option<Kind>,
    spec_from: SpecSource,
    branch: String,
    chain: Vec<Stage>,
    named_files: Vec<String>,
    limit_files: Vec<String>,
    start: Option<String>,
    failed_tail: Option<String>,
    draft_opened: bool,
    extra_contract: String,
    log: Log,
}

pub fn execute(opts: RunOpts) -> Result<i32, RunError> {
    let root = gitutil::toplevel(&opts.cwd).map_err(RunError::new)?;
    license::authorize(&opts.home, &root, gitutil::origin(&root).as_deref())
        .map_err(RunError::new)?;
    let cfg = load_config(&root);
    let id = opts.id.clone().unwrap_or_else(new_run_id);
    let (files, named_spec) = existing_named(&root, &opts.task);
    let run_dir = root.join(".agentfac/runs").join(&id);
    let run_spec_exists = run_dir.join("spec.md").is_file();
    let spec_slices = spec_slice_count(&root, named_spec.as_deref(), &run_dir, run_spec_exists);
    let facts = Facts {
        task: opts.task.clone(),
        kind_flag: opts.kind,
        yes: opts.yes,
        forced_skill: opts.skill.clone(),
        status_clean: gitutil::status_clean(&root).unwrap_or(false),
        branch: gitutil::branch(&root),
        test_command: cfg.test.clone(),
        named_files: files.clone(),
        named_spec: named_spec.clone(),
        run_spec_exists,
        spec_slices,
    };
    match picker::pick(&facts) {
        Decision::Stop { message } => {
            println!("{message}");
            Ok(1)
        }
        Decision::Wait { plan, message } => {
            if opts.dry {
                println!("{message}");
                println!("{}", dry_report(&root, &cfg, &plan, &opts.task, &files)?);
            } else {
                write_run_skeleton(&root, &id, &plan, &opts.task, None)?;
                println!("{message}");
                let log = Log {
                    path: run_dir.join("stages.log"),
                };
                log.line("waiting for --yes");
            }
            Ok(2)
        }
        Decision::Ready(plan) => {
            if opts.dry {
                println!("{}", dry_report(&root, &cfg, &plan, &opts.task, &files)?);
                return Ok(0);
            }
            let mut session = Session::open(root, id, cfg, plan, opts.task, files)?;
            session.ensure_branch()?;
            session.run_chain()?;
            Ok(0)
        }
    }
}

pub fn init_repo(opts: InitOpts) -> Result<(), RunError> {
    let inside = gitutil::toplevel(&opts.cwd).is_ok();
    let origin = if inside {
        gitutil::origin(&opts.cwd)
    } else {
        None
    };
    license::authorize(&opts.home, &opts.cwd, origin.as_deref()).map_err(RunError::new)?;
    if !inside {
        gitutil::init(&opts.cwd).map_err(RunError::new)?;
    }
    let root = gitutil::toplevel(&opts.cwd).map_err(RunError::new)?;
    let path = config_path(&root);
    let mut cfg = if path.is_file() {
        Config::load(&path).map_err(RunError::new)?
    } else {
        Config::default()
    };
    if let Some(agent) = opts.agent {
        cfg.agent = agent;
    }
    if let Some(test) = opts.test {
        cfg.test = test;
    }
    if let Some(skills) = opts.skills_dir {
        cfg.skills_dir = skills;
    }
    cfg.write(&path).map_err(RunError::new)?;
    ensure_gitignore(&root);
    println!("wrote {}", path.display());
    Ok(())
}

pub fn resume(opts: ResumeOpts) -> Result<i32, RunError> {
    let root = gitutil::toplevel(&opts.cwd).map_err(RunError::new)?;
    license::authorize(&opts.home, &root, gitutil::origin(&root).as_deref())
        .map_err(RunError::new)?;
    let cfg = load_config(&root);
    let id = match opts.id {
        Some(id) => id,
        None => latest_run(&root)?,
    };
    let run_dir = root.join(".agentfac/runs").join(&id);
    let text = fs::read_to_string(run_dir.join("slices.md"))
        .map_err(|_| RunError::new(format!("no slices.md for run {id}")))?;
    let mut slices = slices::read_slices(&text);
    let failed: Vec<usize> = slices
        .iter()
        .enumerate()
        .filter(|(_, slice)| slice.status == SliceStatus::Failed)
        .map(|(index, _)| index)
        .collect();
    if failed.is_empty() {
        println!("no failed slices");
        return Ok(0);
    }
    let task = fs::read_to_string(run_dir.join("task.txt")).unwrap_or_default();
    let plan = Plan {
        chain: vec![
            Stage::Tdd,
            Stage::Implement,
            Stage::CodeReview,
            Stage::Unslop,
        ],
        kind: None,
        spec_from: SpecSource::Run,
        branch: gitutil::branch(&root),
        test_command: cfg.test.clone(),
    };
    let mut session = Session::open(root, id, cfg, plan, task, Vec::new())?;
    let mut blocked = Vec::new();
    for index in failed {
        if shares_file(&slices[index], &blocked) {
            slices[index].status = SliceStatus::Skipped;
            slices[index].note = "overlaps a failed slice".into();
            session.write_slices(&slices)?;
            session
                .log
                .line(&format!("slice: {} skipped", slices[index].name));
            continue;
        }
        let slice = slices[index].clone();
        match session.run_one_slice(&slice) {
            Ok(ReviewOutcome::Pass) => {
                slices[index].status = SliceStatus::Passed;
                slices[index].note.clear();
            }
            Ok(ReviewOutcome::Draft) => {
                slices[index].status = SliceStatus::Draft;
                slices[index].note = "draft pull request opened".into();
            }
            Err(err) => {
                slices[index].status = SliceStatus::Failed;
                slices[index].note = err.message.clone();
                blocked.extend(slices[index].files.clone());
                session
                    .log
                    .line(&format!("slice: {} failed: {}", slice.name, err.message));
            }
        }
        session.write_slices(&slices)?;
    }
    Ok(0)
}

pub fn write_license(home: &Path) -> Result<PathBuf, RunError> {
    license::write_key(home).map_err(RunError::new)
}

pub fn default_home() -> PathBuf {
    home_dir()
}

impl Session {
    fn open(
        root: PathBuf,
        id: String,
        cfg: Config,
        plan: Plan,
        task: String,
        named_files: Vec<String>,
    ) -> Result<Self, RunError> {
        let run_dir = root.join(".agentfac/runs").join(&id);
        fs::create_dir_all(&run_dir).map_err(|err| RunError::new(err.to_string()))?;
        let spec = materialize_spec(&root, &run_dir, &plan.spec_from, &task)?;
        let start = gitutil::head(&root);
        if let Some(sha) = &start {
            let _ = fs::write(run_dir.join("start.txt"), format!("{sha}\n"));
        }
        let _ = fs::write(run_dir.join("task.txt"), &task);
        let _ = fs::write(run_dir.join("chain.txt"), chain_file(&plan));
        let log = Log {
            path: run_dir.join("stages.log"),
        };
        log.line(&format!("chain: {}", chain_arrow(&plan.chain)));
        log.line(&format!("run: .agentfac/runs/{id}"));
        Ok(Self {
            root,
            id,
            run_dir,
            cfg,
            task,
            spec,
            kind: plan.kind,
            spec_from: plan.spec_from,
            branch: plan.branch,
            chain: plan.chain,
            named_files,
            limit_files: Vec::new(),
            start,
            failed_tail: None,
            draft_opened: false,
            extra_contract: String::new(),
            log,
        })
    }

    fn ensure_branch(&mut self) -> Result<(), RunError> {
        let current = gitutil::branch(&self.root);
        if current != "main" && current != "master" && current != "HEAD" {
            self.branch = current;
            return Ok(());
        }
        let name = format!("agentfac/{}", self.id);
        gitutil::checkout_new(&self.root, &name).map_err(RunError::new)?;
        self.branch = name;
        self.write_chain();
        self.log.line(&format!("branch: {}", self.branch));
        Ok(())
    }

    fn run_chain(&mut self) -> Result<(), RunError> {
        let mut index = 0;
        while index < self.chain.len() {
            self.reload_spec();
            self.maybe_promote_architect();
            let stage = self.chain[index].clone();
            self.log.line(&format!("{}: start", stage.name()));
            match &stage {
                Stage::Unslop => self.run_unslop()?,
                Stage::CodeReview => {
                    if self.run_review()? == ReviewOutcome::Draft {
                        self.draft_opened = true;
                    }
                }
                Stage::Pr => self.run_pr()?,
                Stage::Architect => {
                    self.run_generic(&Stage::Architect)?;
                    let mut slices = self.collect_slices()?;
                    let _ = slices::write_slices(self.run_dir.join("slices.md").as_path(), &slices);
                    self.run_slice_list(&mut slices)?;
                }
                other => {
                    self.run_generic(other)?;
                }
            }
            self.log.line(&format!("{}: done", stage.name()));
            index += 1;
        }
        Ok(())
    }

    fn run_one_slice(&mut self, slice: &Slice) -> Result<ReviewOutcome, RunError> {
        let old_task = self.task.clone();
        let old_limit = self.limit_files.clone();
        self.task = if slice.detail.is_empty() {
            slice.name.clone()
        } else {
            slice.detail.clone()
        };
        self.limit_files = slice.files.clone();
        self.log.line(&format!("slice: {}", slice.name));
        let result = (|| {
            self.run_generic(&Stage::Tdd)?;
            self.run_generic(&Stage::Implement)?;
            let outcome = self.run_review()?;
            if outcome == ReviewOutcome::Draft {
                self.draft_opened = true;
            }
            self.run_unslop()?;
            Ok(outcome)
        })();
        self.task = old_task;
        self.limit_files = old_limit;
        result
    }

    fn run_slice_list(&mut self, slices: &mut [Slice]) -> Result<(), RunError> {
        let mut blocked = Vec::new();
        for index in 0..slices.len() {
            if shares_file(&slices[index], &blocked) {
                slices[index].status = SliceStatus::Skipped;
                slices[index].note = "overlaps a failed slice".into();
                let name = slices[index].name.clone();
                self.write_slices(&slices.to_vec())?;
                self.log.line(&format!("slice: {name} skipped"));
                continue;
            }
            let current = slices[index].clone();
            match self.run_one_slice(&current) {
                Ok(ReviewOutcome::Pass) => {
                    slices[index].status = SliceStatus::Passed;
                    slices[index].note.clear();
                }
                Ok(ReviewOutcome::Draft) => {
                    slices[index].status = SliceStatus::Draft;
                    slices[index].note = "draft pull request opened".into();
                }
                Err(err) => {
                    slices[index].status = SliceStatus::Failed;
                    slices[index].note = err.message.clone();
                    blocked.extend(slices[index].files.clone());
                    self.log
                        .line(&format!("slice: {} failed: {}", current.name, err.message));
                }
            }
            self.write_slices(&slices.to_vec())?;
        }
        Ok(())
    }

    fn run_generic(&mut self, stage: &Stage) -> Result<String, RunError> {
        let output = self.call_stage(stage)?;
        if matches!(stage, Stage::Tdd | Stage::Implement) {
            self.run_tests();
        }
        if matches!(stage, Stage::ToSpec) {
            self.ensure_spec(&output);
            self.maybe_promote_architect();
        }
        Ok(output)
    }

    fn run_review(&mut self) -> Result<ReviewOutcome, RunError> {
        let first = self.call_stage(&Stage::CodeReview)?;
        self.write_review("review", &first);
        if parse_verdict(&first) == Some(true) {
            self.log.line("code-review: pass");
            return Ok(ReviewOutcome::Pass);
        }
        self.log.line("code-review: fail, one implement retry");
        let note = fs::read_to_string(self.run_dir.join("review.md")).unwrap_or(first);
        self.call_stage_with_note(&note)?;
        self.run_tests();
        let second = self.call_stage(&Stage::CodeReview)?;
        self.write_review("review retry", &second);
        if parse_verdict(&second) == Some(true) {
            self.log.line("code-review: pass");
            return Ok(ReviewOutcome::Pass);
        }
        self.log
            .line("code-review: fail, opening a draft pull request");
        let url = self.open_draft()?;
        self.log.line(&format!("draft: {url}"));
        Ok(ReviewOutcome::Draft)
    }

    fn call_stage_with_note(&mut self, note: &str) -> Result<String, RunError> {
        let stage = Stage::Implement;
        self.log.line("implement: review retry");
        let input = self.pack_input(&stage, Some(note));
        self.call_packs(&stage, &input)
    }

    fn run_unslop(&mut self) -> Result<(), RunError> {
        let files = gitutil::changed_names(&self.root, self.start.as_deref()).unwrap_or_default();
        format_repo(&self.root, &self.cfg.formatter, &files).map_err(RunError::new)?;
        let diff = gitutil::diff_since(&self.root, self.start.as_deref()).unwrap_or_default();
        let marks = noise_marks(&diff);
        if marks.is_empty() {
            self.commit(&files, "agentfac: unslop")?;
            self.log.line("unslop: format clean, skipped model");
            return Ok(());
        }
        let _ = fs::write(
            self.run_dir.join("unslop-mark.txt"),
            marks.join("\n") + "\n",
        );
        for mark in &marks {
            self.log.line(&format!("unslop: {mark}"));
        }
        self.commit(&files, "agentfac: format")?;
        self.extra_contract = format!("Noise still in the diff:\n{}", marks.join("\n"));
        self.call_stage(&Stage::Unslop)?;
        self.extra_contract.clear();
        Ok(())
    }

    fn run_pr(&mut self) -> Result<(), RunError> {
        if self.draft_opened {
            self.log.line("pr: draft already opened, skipped");
            return Ok(());
        }
        self.reload_spec();
        let spec = self.spec.clone().unwrap_or_default();
        let (title, body) = spec_parts(&spec);
        if !title.is_empty() && !body.is_empty() {
            let body_path = self.run_dir.join("pr-body.md");
            fs::write(&body_path, &body).map_err(|err| RunError::new(err.to_string()))?;
            let url = self.open_pull(&title, &body_path, false)?;
            self.log.line(&format!("pr: {url}"));
            return Ok(());
        }
        let output = self.call_stage(&Stage::Pr)?;
        let (model_title, model_body) = parse_pr_text(&output);
        let title = model_title
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| pr_title(&spec, &self.task));
        let body = model_body.unwrap_or(output);
        let body_path = self.run_dir.join("pr-body.md");
        fs::write(&body_path, body.trim()).map_err(|err| RunError::new(err.to_string()))?;
        let url = self.open_pull(&title, &body_path, false)?;
        self.log.line(&format!("pr: {url}"));
        Ok(())
    }

    fn call_stage(&mut self, stage: &Stage) -> Result<String, RunError> {
        let input = self.pack_input(stage, None);
        self.call_packs(stage, &input)
    }

    fn call_packs(&mut self, stage: &Stage, input: &PackInput) -> Result<String, RunError> {
        self.require_agent()?;
        let (skill, skill_notes) = skill::load_skill(
            &self.root.join(".agentfac/cache"),
            &self.cfg.skill_dirs(&self.root),
            stage.name(),
        )
        .map_err(RunError::new)?;
        for note in skill_notes {
            self.log.line(&format!("{}: {note}", stage.name()));
        }
        let packs = pack::build(input).map_err(|err| RunError::new(err.to_string()))?;
        let mut outputs = Vec::new();
        let mut edits = Vec::new();
        for pack in &packs {
            for note in &pack.notes {
                self.log.line(&format!("{}: {note}", stage.name()));
            }
            let model = self.model_for(stage);
            let contract = self.contract(stage, pack.splices.first());
            let (prompt, task_notes) =
                compose_prompt(&skill, &self.task, &pack.text, &model, &contract);
            for note in task_notes {
                self.log.line(&format!("{}: {note}", stage.name()));
            }
            let output = call_agent(&self.root, &self.cfg.agent, &model, &prompt)?;
            if let Some(edit) = extract_edit(&output) {
                for splice in &pack.splices {
                    edits.push((splice.clone(), edit.clone()));
                }
            }
            outputs.push(output);
        }
        self.apply_edits(&edits)?;
        let combined = outputs.join("\n");
        let _ = fs::write(
            self.run_dir.join(format!("{}.out", stage.name())),
            &combined,
        );
        let commit_paths = match parse_paths(&combined) {
            Some(paths) => paths,
            None => gitutil::changed_names(&self.root, self.start.as_deref()).unwrap_or_default(),
        };
        self.commit(&commit_paths, &format!("agentfac: {}", stage.name()))?;
        Ok(combined)
    }

    fn apply_edits(&self, edits: &[(pack::Splice, String)]) -> Result<(), RunError> {
        let mut by_path: Vec<(String, usize, usize, Option<String>, Option<String>)> = Vec::new();
        for (splice, text) in edits {
            if let Some(slot) = by_path
                .iter_mut()
                .find(|(path, _, _, _, _)| path == &splice.path)
            {
                if splice.part == 1 {
                    slot.3 = Some(text.clone());
                } else {
                    slot.4 = Some(text.clone());
                }
            } else {
                by_path.push((
                    splice.path.clone(),
                    splice.start,
                    splice.end,
                    if splice.part == 1 {
                        Some(text.clone())
                    } else {
                        None
                    },
                    if splice.part == 2 {
                        Some(text.clone())
                    } else {
                        None
                    },
                ));
            }
        }
        for (path, start, end, top, bottom) in by_path {
            let (Some(top), Some(bottom)) = (top, bottom) else {
                self.log
                    .line(&format!("span split: {path} missing an edited half"));
                continue;
            };
            let full = self.root.join(&path);
            let original = fs::read_to_string(&full).unwrap_or_default();
            if end > original.len() || start > end {
                continue;
            }
            let mut merged = top;
            if !merged.ends_with('\n') && !bottom.starts_with('\n') {
                merged.push('\n');
            }
            merged.push_str(&bottom);
            let mut out = String::new();
            out.push_str(&original[..start]);
            out.push_str(&merged);
            out.push_str(&original[end..]);
            fs::write(&full, out).map_err(|err| RunError::new(err.to_string()))?;
        }
        Ok(())
    }

    fn run_tests(&mut self) {
        if self.cfg.test.trim().is_empty() {
            self.log.line("test: skipped");
            self.failed_tail = None;
            return;
        }
        match run_command(&self.root, &self.cfg.test, "") {
            Ok(output) => {
                let combined = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                if output.status.success() {
                    self.log.line("test: pass");
                    self.failed_tail = None;
                    let _ = fs::remove_file(self.run_dir.join("test.tail"));
                } else {
                    self.log.line("test: fail");
                    let tail = last_lines(&combined, pack::TEST_TAIL_LINES);
                    self.failed_tail = Some(tail.clone());
                    let _ = fs::write(self.run_dir.join("test.tail"), tail);
                }
            }
            Err(err) => {
                self.log.line(&format!("test: fail {err}"));
                self.failed_tail = Some(err.message);
            }
        }
    }

    fn pack_input(&self, stage: &Stage, review_note: Option<&str>) -> PackInput {
        let mut input = PackInput::new(stage.clone());
        input.spec = self.spec.clone();
        input.symbol = pack::extract_symbol(&format!(
            "{}\n{}",
            self.task,
            self.spec.as_deref().unwrap_or("")
        ));
        input.files = self.files_for(stage, input.symbol.as_deref());
        input.tree = file_tree(&self.root);
        input.readme = read_readme(&self.root);
        input.diff = gitutil::diff_since(&self.root, self.start.as_deref()).ok();
        input.test_command = self.cfg.test.clone();
        input.failed_test_tail = self.failed_tail.clone();
        input.review_note = review_note.map(str::to_string);
        input.context_chars = self.cfg.context_chars;
        input
    }

    fn files_for(&self, stage: &Stage, symbol: Option<&str>) -> Vec<SourceFile> {
        match stage {
            Stage::DiagnosingBugs | Stage::Tdd => self.one_source(true, symbol),
            Stage::Implement => self.implement_files(symbol),
            Stage::Other(_) => self.one_source(true, symbol),
            _ => Vec::new(),
        }
    }

    fn one_source(&self, span: bool, _symbol: Option<&str>) -> Vec<SourceFile> {
        let Some(path) = self.choose_source() else {
            return Vec::new();
        };
        self.read_source(&path, span).into_iter().collect()
    }

    fn implement_files(&self, _symbol: Option<&str>) -> Vec<SourceFile> {
        let mut files = Vec::new();
        if let Some(test) = self.newest_test() {
            if let Some(source) = self.read_source(&test, false) {
                let body = source.contents.clone();
                files.push(source);
                if let Some(prod) = production_file(&self.root, &test, &body) {
                    if prod != test {
                        if let Some(source) = self.read_source(&prod, true) {
                            files.push(source);
                        }
                    }
                } else if let Some(path) = self.choose_source() {
                    if path != test {
                        if let Some(source) = self.read_source(&path, true) {
                            files.push(source);
                        }
                    }
                }
                return files;
            }
        }
        self.one_source(true, None)
    }

    fn choose_source(&self) -> Option<String> {
        let pool: Vec<String> = if self.limit_files.is_empty() {
            let mut paths = self.named_files.clone();
            if let Some(spec) = &self.spec {
                for token in path_tokens(spec) {
                    if self.root.join(&token).is_file() && !paths.iter().any(|item| item == &token)
                    {
                        paths.push(token);
                    }
                }
            }
            paths
        } else {
            self.limit_files
                .iter()
                .filter(|path| self.root.join(path).is_file())
                .cloned()
                .collect()
        };
        pool.iter()
            .find(|path| !path.ends_with(".md") && !is_test_path(path))
            .cloned()
            .or_else(|| pool.iter().find(|path| !path.ends_with(".md")).cloned())
            .or_else(|| pool.first().cloned())
    }

    fn newest_test(&self) -> Option<String> {
        let changed = gitutil::changed_names(&self.root, self.start.as_deref()).ok()?;
        changed.into_iter().find(|path| is_test_path(path))
    }

    fn read_source(&self, path: &str, span: bool) -> Option<SourceFile> {
        let full = self.root.join(path);
        let len = full.metadata().ok()?.len();
        if len > FILE_BYTE_CAP {
            self.log.line(&format!("{path} is over 200KB"));
            return None;
        }
        let contents = fs::read_to_string(&full).ok()?;
        Some(SourceFile {
            path: path.to_string(),
            contents,
            span,
        })
    }

    fn contract(&self, stage: &Stage, splice: Option<&pack::Splice>) -> String {
        let mut text = if let Some(splice) = splice.filter(|item| item.parts == 2) {
            format!(
                "This is part {} of 2 of {}.\nDo not edit other files. Return the edited part between AGENTFAC_EDIT_START and AGENTFAC_EDIT_END.\nAGENTFAC_PATHS: {}",
                splice.part, splice.path, splice.path
            )
        } else {
            match stage {
                Stage::CodeReview => "End with a line that is exactly pass or fail.".into(),
                Stage::ToSpec => format!("Write the spec to .agentfac/runs/{}/spec.md.", self.id),
                Stage::Architect => {
                    "Write slices after a line AGENTFAC_SLICES: using `- name | files: a, b | detail`."
                        .into()
                }
                Stage::Pr => "Print AGENTFAC_TITLE: and AGENTFAC_BODY:. Do not include the diff.".into(),
                _ => "Edit the repo. On the last line print AGENTFAC_PATHS: path, path.".into(),
            }
        };
        if !self.extra_contract.trim().is_empty() {
            text.push('\n');
            text.push_str(self.extra_contract.trim());
        }
        text
    }

    fn require_agent(&self) -> Result<(), RunError> {
        if self.cfg.agent.trim().is_empty() {
            return Err(RunError::new(
                "agent command is empty in .agentfac/config.toml. Set agent to a command that reads a prompt on stdin.",
            ));
        }
        Ok(())
    }

    fn model_for(&self, stage: &Stage) -> String {
        if matches!(stage, Stage::Unslop) && !self.cfg.unslop_model.is_empty() {
            self.cfg.unslop_model.clone()
        } else {
            self.cfg.model.clone()
        }
    }

    fn commit(&self, paths: &[String], message: &str) -> Result<(), RunError> {
        match gitutil::commit_paths(&self.root, paths, message) {
            Ok(Some(sha)) => {
                self.log.line(&format!("commit: {sha}"));
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(err) => Err(RunError::new(err)),
        }
    }

    fn reload_spec(&mut self) {
        let path = self.run_dir.join("spec.md");
        if let Ok(text) = fs::read_to_string(path) {
            if !text.trim().is_empty() {
                self.spec = Some(text);
            }
        }
    }

    fn ensure_spec(&mut self, output: &str) {
        let path = self.run_dir.join("spec.md");
        let existing = fs::read_to_string(&path).unwrap_or_default();
        if existing.trim().is_empty() {
            let body = output
                .lines()
                .filter(|line| !line.trim().starts_with("AGENTFAC_"))
                .collect::<Vec<_>>()
                .join("\n");
            if !body.trim().is_empty() {
                let _ = fs::write(&path, body.trim());
            }
        }
        self.reload_spec();
    }

    fn maybe_promote_architect(&mut self) {
        let Some(spec) = self.spec.clone() else {
            return;
        };
        if slices::count_slices(&spec) < 2 {
            return;
        }
        if let Some(Stage::Pr) = self.chain.last() {
            self.chain.pop();
            self.chain.push(Stage::Architect);
            self.write_chain();
            self.log
                .line("spec lists two or more slices, ship step is architect");
        }
    }

    fn collect_slices(&mut self) -> Result<Vec<Slice>, RunError> {
        self.reload_spec();
        let output = fs::read_to_string(self.run_dir.join("architect.out")).unwrap_or_default();
        let mut found = slices::parse_agent_slices(&output);
        if found.len() < 2 {
            if let Some(spec) = &self.spec {
                let from_spec = slices::slices_from_spec(spec);
                if from_spec.len() > found.len() {
                    found = from_spec;
                }
            }
        }
        if found.len() < 2 {
            return Err(RunError::new("architect did not list two slices"));
        }
        Ok(found)
    }

    fn write_slices(&self, slices: &[Slice]) -> Result<(), RunError> {
        slices::write_slices(&self.run_dir.join("slices.md"), slices)
            .map_err(|err| RunError::new(err.to_string()))
    }

    fn write_chain(&self) {
        let plan = Plan {
            chain: self.chain.clone(),
            kind: self.kind,
            spec_from: self.spec_from.clone(),
            branch: self.branch.clone(),
            test_command: self.cfg.test.clone(),
        };
        let _ = fs::write(self.run_dir.join("chain.txt"), chain_file(&plan));
    }

    fn write_review(&self, heading: &str, output: &str) {
        let path = self.run_dir.join("review.md");
        let mut text = fs::read_to_string(&path).unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("## {heading}\n{output}\n"));
        let _ = fs::write(path, text);
    }

    fn open_draft(&self) -> Result<String, RunError> {
        let spec = self.spec.clone().unwrap_or_default();
        let title = pr_title(&spec, &self.task);
        let review = self.run_dir.join("review.md");
        self.open_pull(&title, &review, true)
    }

    fn open_pull(&self, title: &str, body: &Path, draft: bool) -> Result<String, RunError> {
        if gitutil::origin(&self.root).is_none() {
            let copy = self
                .run_dir
                .join(if draft { "pr-draft.md" } else { "pr.md" });
            if let Ok(text) = fs::read_to_string(body) {
                let _ = fs::write(&copy, text);
            }
            return Ok(format!("no git remote; wrote {}", copy.display()));
        }
        gitutil::push_head(&self.root).map_err(RunError::new)?;
        let gh = std::env::var("AGENTFAC_GH").unwrap_or_else(|_| "gh".into());
        let mut command = format!(
            "{gh} pr create {} --title {} --body-file {}",
            if draft { "--draft" } else { "" },
            shell_quote(title),
            shell_quote(&body.display().to_string()),
        );
        command = command.replace("  ", " ");
        let output = run_command(&self.root, &command, "")?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(RunError::new(format!(
                "gh pr create failed: {}",
                err.trim()
            )));
        }
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if url.is_empty() {
            Ok("pull request opened".into())
        } else {
            Ok(url)
        }
    }
}

fn dry_report(
    root: &Path,
    cfg: &Config,
    plan: &Plan,
    task: &str,
    named_files: &[String],
) -> Result<String, RunError> {
    let mut session_spec = None;
    if let SpecSource::File(path) = &plan.spec_from {
        session_spec = fs::read_to_string(root.join(path)).ok();
    } else if matches!(plan.spec_from, SpecSource::Task) {
        session_spec = Some(task.to_string());
    }
    let mut out = String::new();
    out.push_str(&format!("chain: {}\n", chain_arrow(&plan.chain)));
    out.push_str(&format!("branch: {}\n", gitutil::branch(root)));
    out.push_str(&format!(
        "test: {}\n",
        if cfg.test.is_empty() {
            "-"
        } else {
            cfg.test.as_str()
        }
    ));
    for stage in &plan.chain {
        out.push_str(&format!("\n{}\n", stage.name()));
        if find_skill(&cfg.skill_dirs(root), stage.name()).is_none() && !matches!(stage, Stage::Pr)
        {
            out.push_str("  note: skill file missing\n");
        }
        let mut input = PackInput::new(stage.clone());
        input.spec = session_spec.clone();
        input.symbol = pack::extract_symbol(&format!(
            "{task}\n{}",
            session_spec.as_deref().unwrap_or("")
        ));
        input.context_chars = cfg.context_chars;
        input.test_command = cfg.test.clone();
        input.tree = file_tree(root);
        input.readme = read_readme(root);
        input.diff = gitutil::diff_since(root, gitutil::head(root).as_deref()).ok();
        if matches!(
            stage,
            Stage::DiagnosingBugs | Stage::Tdd | Stage::Implement | Stage::Other(_)
        ) {
            if let Some(path) = named_files.iter().find(|path| !path.ends_with(".md")) {
                if let Some(source) = read_optional(root, path, true) {
                    input.files.push(source);
                }
            }
        }
        if matches!(stage, Stage::Implement) {
            out.push_str("  note: test file after tdd\n");
        }
        match pack::build(&input) {
            Ok(packs) => {
                let mut files = Vec::new();
                let mut notes = Vec::new();
                for pack in &packs {
                    for file in &pack.files {
                        if !files.contains(file) {
                            files.push(file.clone());
                        }
                    }
                    notes.extend(pack.notes.clone());
                }
                let label = match stage {
                    Stage::CodeReview | Stage::Unslop => "diff".to_string(),
                    _ if files.is_empty() => "none".to_string(),
                    _ => files.join(", "),
                };
                out.push_str(&format!("  files: {label}\n"));
                for note in notes {
                    out.push_str(&format!("  note: {note}\n"));
                }
            }
            Err(err) => out.push_str(&format!("  error: {err}\n")),
        }
    }
    Ok(out)
}

fn read_optional(root: &Path, path: &str, span: bool) -> Option<SourceFile> {
    let full = root.join(path);
    if full.metadata().ok()?.len() > FILE_BYTE_CAP {
        return None;
    }
    Some(SourceFile {
        path: path.to_string(),
        contents: fs::read_to_string(full).ok()?,
        span,
    })
}

fn materialize_spec(
    root: &Path,
    run_dir: &Path,
    source: &SpecSource,
    task: &str,
) -> Result<Option<String>, RunError> {
    let dest = run_dir.join("spec.md");
    match source {
        SpecSource::File(path) => {
            let text = fs::read_to_string(root.join(path)).unwrap_or_default();
            if !dest.is_file() {
                let _ = fs::write(&dest, &text);
            }
            Ok(Some(text))
        }
        SpecSource::Task => {
            if !dest.is_file() {
                let _ = fs::write(&dest, task);
            }
            Ok(Some(task.to_string()))
        }
        SpecSource::Run => {
            let text = fs::read_to_string(&dest).unwrap_or_default();
            if text.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(text))
            }
        }
        SpecSource::None => Ok(None),
    }
}

fn spec_slice_count(
    root: &Path,
    named_spec: Option<&str>,
    run_dir: &Path,
    run_spec: bool,
) -> usize {
    if let Some(path) = named_spec {
        return slices::count_slices(&fs::read_to_string(root.join(path)).unwrap_or_default());
    }
    if run_spec {
        return slices::count_slices(
            &fs::read_to_string(run_dir.join("spec.md")).unwrap_or_default(),
        );
    }
    0
}

fn existing_named(root: &Path, text: &str) -> (Vec<String>, Option<String>) {
    let mut files = Vec::new();
    let mut spec = None;
    for token in path_tokens(text) {
        if root.join(&token).is_file() && !files.iter().any(|item| item == &token) {
            if spec.is_none() && token.ends_with(".md") {
                spec = Some(token.clone());
            }
            files.push(token);
        }
    }
    (files, spec)
}

fn write_run_skeleton(
    root: &Path,
    id: &str,
    plan: &Plan,
    task: &str,
    _start: Option<&str>,
) -> Result<(), RunError> {
    let run_dir = root.join(".agentfac/runs").join(id);
    fs::create_dir_all(&run_dir).map_err(|err| RunError::new(err.to_string()))?;
    fs::write(run_dir.join("chain.txt"), chain_file(plan))
        .map_err(|err| RunError::new(err.to_string()))?;
    fs::write(run_dir.join("task.txt"), task).map_err(|err| RunError::new(err.to_string()))?;
    Ok(())
}

fn load_config(root: &Path) -> Config {
    let path = config_path(root);
    if path.is_file() {
        Config::load(&path).unwrap_or_default()
    } else {
        Config::default()
    }
}

fn ensure_gitignore(root: &Path) {
    let path = root.join(".gitignore");
    let mut text = fs::read_to_string(&path).unwrap_or_default();
    for line in [".agentfac/runs/", ".agentfac/cache/"] {
        if !text.lines().any(|existing| existing.trim() == line) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(line);
            text.push('\n');
        }
    }
    let _ = fs::write(path, text);
}

fn latest_run(root: &Path) -> Result<String, RunError> {
    let dir = root.join(".agentfac/runs");
    let mut best: Option<(SystemTime, String)> = None;
    let entries = fs::read_dir(&dir).map_err(|_| RunError::new("no runs to resume"))?;
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(UNIX_EPOCH);
        if best
            .as_ref()
            .map(|(time, _)| modified >= *time)
            .unwrap_or(true)
        {
            best = Some((modified, name));
        }
    }
    best.map(|(_, name)| name)
        .ok_or_else(|| RunError::new("no runs to resume"))
}

fn new_run_id() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut bytes = [0u8; 2];
    if let Ok(mut file) = fs::File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    }
    format!("r{secs:010}-{:02x}{:02x}", bytes[0], bytes[1])
}

fn pr_title(spec: &str, task: &str) -> String {
    let (title, _) = spec_parts(spec);
    let mut title = if title.is_empty() {
        task.trim().to_string()
    } else {
        title
    };
    if let Some(line) = title.lines().next() {
        title = line.trim().to_string();
    }
    if crate::text::char_count(&title) > 72 {
        title = crate::text::prefix_chars(&title, 72).to_string();
    }
    if title.is_empty() {
        "agentfac".into()
    } else {
        title
    }
}

fn is_test_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.contains("/tests/")
        || path.contains("/__tests__/")
        || path.contains("/test/")
        || name.starts_with("test_")
        || name.ends_with("_test.rs")
        || name.ends_with("_test.go")
        || name.contains(".test.")
        || name.contains(".spec.")
}

fn production_file(root: &Path, test_path: &str, body: &str) -> Option<String> {
    for target in import_targets(body) {
        for candidate in resolve_import(test_path, &target) {
            if candidate != test_path && root.join(&candidate).is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn import_targets(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("use ") {
            let rest = rest.trim().trim_end_matches(';').trim();
            if let Some(path) = rest.strip_prefix("crate::") {
                let module = path.split("::").next().unwrap_or(path);
                if !module.is_empty() {
                    out.push(format!("src/{module}.rs"));
                    out.push(format!("src/{module}/mod.rs"));
                }
            }
        }
        push_quoted(&mut out, line, "from ");
        push_quoted(&mut out, line, "import ");
        push_quoted(&mut out, line, "require(");
        if let Some(rest) = line.strip_prefix("import ") {
            if !rest.contains('\'') && !rest.contains('"') {
                if let Some(module) = rest.split([',', ' ', '.']).next() {
                    if !module.is_empty() {
                        out.push(format!("{module}.py"));
                    }
                }
            }
        }
        if let Some(rest) = line.strip_prefix("from ") {
            if !rest.contains('\'') && !rest.contains('"') {
                if let Some(module) = rest.split_whitespace().next() {
                    if !module.is_empty() {
                        out.push(format!("{}.py", module.replace('.', "/")));
                    }
                }
            }
        }
    }
    out
}

fn push_quoted(out: &mut Vec<String>, line: &str, prefix: &str) {
    let Some(start) = line.find(prefix) else {
        return;
    };
    let after = &line[start + prefix.len()..];
    let Some(quote) = after.chars().next() else {
        return;
    };
    if quote != '\'' && quote != '"' {
        return;
    }
    let rest = &after[quote.len_utf8()..];
    if let Some(end) = rest.find(quote) {
        out.push(rest[..end].to_string());
    }
}

fn resolve_import(test_path: &str, target: &str) -> Vec<String> {
    let base = if target.starts_with('.') {
        let dir = Path::new(test_path)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        dir.join(target).to_string_lossy().replace('\\', "/")
    } else {
        target.to_string()
    };
    let mut out = vec![base.clone()];
    for ext in [
        ".rs",
        ".ts",
        ".tsx",
        ".js",
        ".jsx",
        ".py",
        "/mod.rs",
        "/index.ts",
        "/index.js",
    ] {
        out.push(format!("{base}{ext}"));
    }
    out
}

fn call_agent(repo: &Path, agent: &str, model: &str, prompt: &str) -> Result<String, RunError> {
    if agent.trim().is_empty() {
        return Err(RunError::new(
            "agent command is empty in .agentfac/config.toml. Set agent to a command that reads a prompt on stdin.",
        ));
    }
    let command = agent.replace("{model}", model);
    let output = run_command_with_model(repo, &command, prompt, model)?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(RunError::new(format!(
            "agent exited {}: {}",
            output.status,
            last_lines(&err, 40)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_command(repo: &Path, command: &str, input: &str) -> Result<std::process::Output, RunError> {
    run_command_with_model(repo, command, input, "")
}

fn run_command_with_model(
    repo: &Path,
    command: &str,
    input: &str,
    model: &str,
) -> Result<std::process::Output, RunError> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(repo)
        .env("AGENTFAC_MODEL", model)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|err| RunError::new(format!("failed to start command: {err}")))?;
    let writer = child.stdin.take().map(|mut stdin| {
        let owned = input.to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(owned.as_bytes());
        })
    });
    let output = child
        .wait_with_output()
        .map_err(|err| RunError::new(err.to_string()))?;
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    Ok(output)
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
