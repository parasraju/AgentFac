# Agentfac

Agentfac picks a skill chain locally, then runs that chain on the git repo you have open. Each stage shells out to the agent command in `.agentfac/config.toml` and sends one trimmed skill file. The CLI runs tests and commits. It does not call a model to choose the chain.

```text
agentfac init --agent "your-agent"
agentfac run "update src/app.rs to log the retry"
agentfac run --dry "add retry to the client"
agentfac run --yes "add retry to the client"
agentfac skills
agentfac resume
```

Put flags before the task text. `--kind bug` or `--kind change` wins over keywords. A vague task, or one that reads as both a bug and a change, prints the guessed chain and waits until `--yes`.

The agent command reads the prompt on stdin. Point `skills_dir` at skill directories that contain `<name>.md` or `<name>/SKILL.md`. The default chain uses Matt Pocock's `diagnosing-bugs`, `to-spec`, `tdd`, `implement`, `code-review`, and `pr`, plus pstack `unslop` and, when the spec lists two or more slices, pstack `architect`. Skill text is not built into the binary.

```toml
agent = "your-agent"
test = "cargo test"
context_chars = 24000
model = ""
unslop_model = ""
formatter = ""
skills_dir = ""
```

`model` is passed through as `AGENTFAC_MODEL` and replaces `{model}` in the agent command. `unslop_model` is used only for the unslop stage. A local format pass runs first. The unslop model runs only when that pass leaves a noise mark.

The first repo you init is recorded locally. A second repo needs `~/.agentfac/license.key`. `agentfac license` writes that file. There is no account. Set `AGENTFAC_HOME` to move the key and the repo list.

`code-review` runs `implement` once more when the review says `fail`, then reviews once more. A second failure opens a draft pull request with `review.md` and the run continues. `agentfac resume` reruns failed slices and skips a later slice that names a file from a failed one.
