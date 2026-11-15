use std::path::PathBuf;

use crate::picker::{skills_text, Kind};
use crate::runner::{self, InitOpts, ResumeOpts, RunOpts};

pub fn entry() -> i32 {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        eprintln!("{}", help_text());
        return 1;
    }
    if args[0] == "--help" || args[0] == "-h" || args[0] == "help" {
        print!("{}", help_text());
        return 0;
    }
    let home = runner::default_home();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let command = args.remove(0);
    let result = match command.as_str() {
        "skills" => {
            print!("{}", skills_text());
            Ok(0)
        }
        "license" => runner::write_license(&home).map(|path| {
            println!("wrote {}", path.display());
            0
        }),
        "init" => match parse_init(&args) {
            Ok((agent, test, skills_dir)) => runner::init_repo(InitOpts {
                cwd,
                home,
                agent,
                test,
                skills_dir,
            })
            .map(|()| 0),
            Err(message) => Err(runner::RunError::new(message)),
        },
        "resume" => {
            if args.len() > 1 {
                Err(runner::RunError::new("usage: agentfac resume [id]"))
            } else {
                runner::resume(ResumeOpts {
                    cwd,
                    home,
                    id: args.first().cloned(),
                })
            }
        }
        "run" => match parse_run(&args) {
            Ok(opts) => runner::execute(RunOpts {
                cwd,
                home,
                task: opts.task,
                dry: opts.dry,
                yes: opts.yes,
                kind: opts.kind,
                skill: opts.skill,
                id: opts.id,
            }),
            Err(message) => Err(runner::RunError::new(message)),
        },
        other => Err(runner::RunError::new(format!(
            "unknown command `{other}`\n{}",
            help_text()
        ))),
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

struct RunFlags {
    dry: bool,
    yes: bool,
    kind: Option<Kind>,
    skill: Option<String>,
    id: Option<String>,
    task: String,
}

fn parse_run(args: &[String]) -> Result<RunFlags, String> {
    let mut dry = false;
    let mut yes = false;
    let mut kind = None;
    let mut skill = None;
    let mut id = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        if !arg.starts_with('-') {
            break;
        }
        match arg.as_str() {
            "--dry" => dry = true,
            "--yes" => yes = true,
            "--kind" => {
                index += 1;
                let value = args.get(index).ok_or("--kind needs bug or change")?;
                kind = Some(Kind::parse(value).ok_or("--kind must be bug or change")?);
            }
            "--skill" => {
                index += 1;
                skill = Some(args.get(index).ok_or("--skill needs a name")?.clone());
            }
            "--id" => {
                index += 1;
                id = Some(args.get(index).ok_or("--id needs a value")?.clone());
            }
            "--help" | "-h" => return Err(help_text()),
            other => return Err(format!("unknown flag {other}")),
        }
        index += 1;
    }
    Ok(RunFlags {
        dry,
        yes,
        kind,
        skill,
        id,
        task: args[index..].join(" "),
    })
}

fn parse_init(args: &[String]) -> Result<(Option<String>, Option<String>, Option<String>), String> {
    let mut agent = None;
    let mut test = None;
    let mut skills_dir = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let value = || {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--agent" => agent = Some(value()?),
            "--test" => test = Some(value()?),
            "--skills-dir" => skills_dir = Some(value()?),
            other => return Err(format!("unknown flag {other}")),
        }
        index += 2;
    }
    Ok((agent, test, skills_dir))
}

fn help_text() -> String {
    "\
agentfac picks a skill chain locally and runs it on this repo.

Usage:
  agentfac run [--dry] [--yes] [--kind bug|change] [--skill name] [--id id] <task>
  agentfac skills
  agentfac init [--agent command] [--test command] [--skills-dir path]
  agentfac resume [id]
  agentfac license

Put flags before the task text.
"
    .to_string()
}
