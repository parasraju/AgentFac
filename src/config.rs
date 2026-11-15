use std::fs;
use std::path::{Path, PathBuf};

use crate::pack::CONTEXT_CAP_DEFAULT;

#[derive(Clone, Debug)]
pub struct Config {
    pub agent: String,
    pub test: String,
    pub context_chars: usize,
    pub model: String,
    pub unslop_model: String,
    pub formatter: String,
    pub skills_dir: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent: String::new(),
            test: String::new(),
            context_chars: CONTEXT_CAP_DEFAULT,
            model: String::new(),
            unslop_model: String::new(),
            formatter: String::new(),
            skills_dir: String::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|err| err.to_string())?;
        Ok(Self::parse(&text))
    }

    pub fn parse(text: &str) -> Self {
        let mut cfg = Config::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = unquote(value.trim());
            match key {
                "agent" => cfg.agent = value,
                "test" => cfg.test = value,
                "context_chars" => {
                    cfg.context_chars = value.parse().unwrap_or(CONTEXT_CAP_DEFAULT);
                }
                "model" => cfg.model = value,
                "unslop_model" => cfg.unslop_model = value,
                "formatter" => cfg.formatter = value,
                "skills_dir" => cfg.skills_dir = value,
                _ => {}
            }
        }
        if cfg.context_chars == 0 {
            cfg.context_chars = CONTEXT_CAP_DEFAULT;
        }
        cfg
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let text = format!(
            "# The agent command reads the prompt on stdin.\nagent = {agent}\ntest = {test}\ncontext_chars = {context}\nmodel = {model}\nunslop_model = {unslop}\nformatter = {formatter}\nskills_dir = {skills}\n",
            agent = toml_string(&self.agent),
            test = toml_string(&self.test),
            context = self.context_chars,
            model = toml_string(&self.model),
            unslop = toml_string(&self.unslop_model),
            formatter = toml_string(&self.formatter),
            skills = toml_string(&self.skills_dir),
        );
        fs::write(path, text).map_err(|err| err.to_string())
    }

    pub fn skill_dirs(&self, root: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if !self.skills_dir.is_empty() {
            dirs.push(PathBuf::from(&self.skills_dir));
        }
        if let Ok(env) = std::env::var("AGENTFAC_SKILLS") {
            for part in env.split(':') {
                if !part.is_empty() {
                    dirs.push(PathBuf::from(part));
                }
            }
        }
        dirs.push(root.join(".agentfac/skills"));
        dirs.push(root.join("skills"));
        dirs
    }
}

pub fn config_path(root: &Path) -> PathBuf {
    root.join(".agentfac/config.toml")
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn unquote(value: &str) -> String {
    let value = value.trim().trim_end_matches(',');
    if let Some(inner) = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    if let Some(inner) = value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return inner.to_string();
    }
    value.to_string()
}
