//! Command results, rendered either as human text or as JSON.

use qjson::{Json, JsonMap};

/// The outcome of one `xtask` command.
#[derive(Clone, Debug)]
pub struct Report {
    /// The command name, e.g. `"validate-knowledge"`.
    pub command: String,
    /// Whether the command succeeded.
    pub ok: bool,
    /// Human-readable lines describing what happened.
    pub notes: Vec<String>,
    /// Problems found. A non-empty list always means `ok == false`.
    pub problems: Vec<Json>,
    /// Structured detail: counts, hashes, file lists.
    pub details: JsonMap,
}

impl Report {
    /// A fresh successful report for `command`.
    pub fn new(command: &str) -> Report {
        Report {
            command: command.to_string(),
            ok: true,
            notes: Vec::new(),
            problems: Vec::new(),
            details: JsonMap::new(),
        }
    }

    /// Adds a human-readable line.
    pub fn note(&mut self, line: impl Into<String>) -> &mut Report {
        self.notes.push(line.into());
        self
    }

    /// Records a problem and marks the report failed.
    pub fn problem(&mut self, path: impl Into<String>, message: impl Into<String>) -> &mut Report {
        let mut m = JsonMap::new();
        m.insert("path", Json::Str(path.into()));
        m.insert("message", Json::Str(message.into()));
        self.problems.push(Json::Obj(m));
        self.ok = false;
        self
    }

    /// Records an already-structured problem.
    pub fn problem_json(&mut self, v: Json) -> &mut Report {
        self.problems.push(v);
        self.ok = false;
        self
    }

    /// Adds a structured detail field.
    pub fn detail(&mut self, key: &str, value: Json) -> &mut Report {
        self.details.insert(key, value);
        self
    }

    /// Folds a sub-report into this one, prefixing its notes.
    pub fn absorb(&mut self, sub: Report) -> &mut Report {
        for n in &sub.notes {
            self.notes.push(format!("  [{}] {n}", sub.command));
        }
        for p in sub.problems {
            self.problems.push(p);
        }
        self.details
            .insert(sub.command.clone(), Json::Obj(sub.details));
        if !sub.ok {
            self.ok = false;
        }
        self
    }

    /// The JSON form emitted by `--json`.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("command", Json::Str(self.command.clone()));
        m.insert("ok", Json::Bool(self.ok));
        m.insert(
            "notes",
            Json::Arr(self.notes.iter().map(|n| Json::Str(n.clone())).collect()),
        );
        m.insert("problems", Json::Arr(self.problems.clone()));
        m.insert("details", Json::Obj(self.details.clone()));
        Json::Obj(m)
    }

    /// Prints the report and returns the process exit code.
    pub fn emit(&self, json: bool) -> i32 {
        if json {
            println!("{}", self.to_json().to_string_pretty());
        } else {
            for n in &self.notes {
                println!("{n}");
            }
            if self.problems.is_empty() {
                println!("{}: ok", self.command);
            } else {
                eprintln!("\n{}: {} problem(s)", self.command, self.problems.len());
                for p in &self.problems {
                    let path = p.get("path").and_then(Json::as_str).unwrap_or("");
                    let message = p.get("message").and_then(Json::as_str).unwrap_or("");
                    let code = p.get("code").and_then(Json::as_str).unwrap_or("");
                    if code.is_empty() {
                        eprintln!("  {path}: {message}");
                    } else {
                        eprintln!("  [{code}] {path}: {message}");
                    }
                }
            }
        }
        i32::from(!self.ok)
    }
}

/// Wraps a `usize` for a detail field.
pub fn count(n: usize) -> Json {
    Json::Int(n as i64)
}

/// Wraps a string slice list for a detail field.
pub fn strings<S: AsRef<str>>(items: &[S]) -> Json {
    Json::Arr(
        items
            .iter()
            .map(|s| Json::Str(s.as_ref().to_string()))
            .collect(),
    )
}
