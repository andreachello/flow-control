//! Work the page can start, so nobody needs a terminal for it.
//!
//! A task comes from a fixed list, so nothing a browser sends reaches a command line. What does the
//! work is a [`Runner`]. A run outlives the page that started it, so someone can leave and come
//! back to it.

use crate::journeys;
use crate::soba;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Journeys,
    Titles,
}

impl Task {
    pub fn named(s: &str) -> Option<Task> {
        match s {
            "journeys" => Some(Task::Journeys),
            "titles" => Some(Task::Titles),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Task::Journeys => "journeys",
            Task::Titles => "titles",
        }
    }
}

/// What does a task for a project. An error is shown to the person who pressed the button, so it
/// should read as one plain sentence.
pub trait Runner: Send + Sync {
    /// `user` is whose Soba compute the task uses, as the sign-in named them. None uses this machine's agent.
    /// `step` hears where it has got to, for the page to show.
    fn run(&self, root: &Path, task: Task, user: Option<&str>, step: &dyn Fn(&str)) -> Result<()>;
}

/// This machine, as this same program, on its owner's own agent sign-in.
pub struct Here {
    pub out: String,
}

impl Runner for Here {
    fn run(&self, root: &Path, task: Task, user: Option<&str>, step: &dyn Fn(&str)) -> Result<()> {
        let mut c = Command::new(std::env::current_exe()?);
        c.arg("-C").arg(root).args(["--out", &self.out, task.word()]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).env_remove(soba::USER);
        if let Some(u) = user {
            c.env(soba::USER, u);
        }
        let mut going = c.spawn().context("could not start flow")?;
        let (out, mut err) = (going.stdout.take().context("no stdout")?, going.stderr.take().context("no stderr")?);
        // Read at the same time as the output below, so neither can fill up and stop the task.
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = err.read_to_string(&mut text);
            text
        });
        let mut said = None;
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            match line.strip_prefix(journeys::STEP) {
                Some(s) => step(s),
                None if !line.trim().is_empty() => said = Some(line.trim().to_string()),
                None => {}
            }
        }
        if going.wait()?.success() {
            return Ok(());
        }
        // A task that finished most of its work names what it left on stdout, and exits 1.
        let errors = errors.join().unwrap_or_default();
        let said = errors.lines().rev().map(str::trim).find(|l| !l.is_empty()).map(str::to_string).or(said).unwrap_or_else(|| "it stopped without saying why".into());
        bail!("{}", said.trim_start_matches("flow: ").trim_start_matches("failed: "))
    }
}

struct Run {
    task: Task,
    started: Instant,
    /// Where it has got to, once it has said.
    step: Option<String>,
    /// How long it took, and what went wrong if anything did. None while it runs.
    ended: Option<(u64, Option<String>)>,
}

impl Run {
    fn json(&self) -> Value {
        let (secs, failed) = match &self.ended {
            Some((s, e)) => (*s, e.as_deref()),
            None => (self.started.elapsed().as_secs(), None),
        };
        // The code is what the page branches on. A refusal's own text names internal ids.
        let code = failed.and_then(soba::refusal).map(|(c, _)| c);
        json!({ "task": self.task.word(), "running": self.ended.is_none(), "failed": failed, "code": code, "secs": secs, "step": self.step })
    }
}

/// The latest run of each project. One at a time per project: two would write the same files.
pub struct Runs {
    roots: HashMap<String, PathBuf>,
    runner: Arc<dyn Runner>,
    latest: Arc<Mutex<HashMap<String, Run>>>,
}

impl Runs {
    pub fn new(roots: HashMap<String, PathBuf>, runner: Arc<dyn Runner>) -> Runs {
        Runs { roots, runner, latest: Arc::default() }
    }

    /// Start a task, or hand back the run already going, so a second click starts nothing.
    pub fn start(&self, project: &str, task: &str, user: Option<String>) -> Result<Value> {
        let root = self.roots.get(project).ok_or_else(|| anyhow!("no such project"))?.clone();
        let task = Task::named(task).context("no such task")?;
        let mut latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = latest.get(project).filter(|r| r.ended.is_none()) {
            return Ok(json!({ "run": r.json() }));
        }
        let run = Run { task, started: Instant::now(), step: None, ended: None };
        let shown = run.json();
        latest.insert(project.to_string(), run);
        let (runner, all, name) = (self.runner.clone(), self.latest.clone(), project.to_string());
        std::thread::spawn(move || {
            let step = |say: &str| {
                if let Some(r) = all.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&name) {
                    r.step = Some(say.to_string());
                }
            };
            let failed = runner.run(&root, task, user.as_deref(), &step).err().map(|e| format!("{e:#}"));
            if let Some(r) = all.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&name) {
                r.ended = Some((r.started.elapsed().as_secs(), failed));
            }
        });
        Ok(json!({ "run": shown }))
    }

    pub fn show(&self, project: &str) -> Result<Value> {
        if !self.roots.contains_key(project) {
            bail!("no such project");
        }
        let latest = self.latest.lock().unwrap_or_else(|e| e.into_inner());
        Ok(json!({ "run": latest.get(project).map(Run::json) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::Duration;

    /// Stands in for whatever does the work, and holds each run open until the test lets it end.
    struct Held {
        began: Mutex<Sender<Task>>,
        end: Mutex<Receiver<Result<(), String>>>,
    }

    impl Runner for Held {
        fn run(&self, _: &Path, task: Task, _: Option<&str>, step: &dyn Fn(&str)) -> Result<()> {
            step("Halfway");
            self.began.lock().unwrap().send(task).unwrap();
            self.end.lock().unwrap().recv().unwrap().map_err(|e| anyhow!(e))
        }
    }

    fn runs() -> (Runs, Receiver<Task>, Sender<Result<(), String>>) {
        let ((began, seen), (finish, end)) = (channel(), channel());
        let held = Held { began: Mutex::new(began), end: Mutex::new(end) };
        (Runs::new(HashMap::from([("shop".to_string(), PathBuf::from("/shop"))]), Arc::new(held)), seen, finish)
    }

    fn settled(r: &Runs) -> Value {
        for _ in 0..500 {
            let v = r.show("shop").unwrap();
            if v["run"]["running"] == false {
                return v;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("the run never ended");
    }

    #[test]
    fn a_second_click_joins_the_run_and_starts_nothing() {
        let (r, seen, finish) = runs();
        assert_eq!(r.show("shop").unwrap()["run"], Value::Null);
        assert_eq!(r.start("shop", "journeys", None).unwrap()["run"]["running"], true);
        assert_eq!(seen.recv().unwrap(), Task::Journeys);
        assert_eq!(r.show("shop").unwrap()["run"]["step"], "Halfway", "the page can see where it has got to");
        let again = r.start("shop", "titles", None).unwrap();
        assert_eq!(again["run"]["task"], "journeys", "{again}");
        finish.send(Ok(())).unwrap();
        assert_eq!(settled(&r)["run"]["failed"], Value::Null);
        assert!(seen.try_recv().is_err(), "the second click must not have started anything");
    }

    #[test]
    fn a_failure_stays_on_show_and_another_try_can_start() {
        let (r, seen, finish) = runs();
        r.start("shop", "journeys", None).unwrap();
        seen.recv().unwrap();
        finish.send(Err("could not start `claude`".into())).unwrap();
        assert_eq!(settled(&r)["run"]["failed"], "could not start `claude`");
        assert_eq!(r.start("shop", "journeys", None).unwrap()["run"]["running"], true);
        seen.recv().unwrap();
        finish.send(Ok(())).unwrap();
        assert_eq!(settled(&r)["run"]["failed"], Value::Null);
    }

    #[test]
    fn only_listed_tasks_and_known_projects_run() {
        let (r, seen, _finish) = runs();
        for (project, task) in [("shop", "rm -rf"), ("shop", ""), ("nobody", "journeys")] {
            assert!(r.start(project, task, None).is_err(), "{project} {task}");
        }
        assert!(r.show("nobody").is_err());
        assert!(seen.try_recv().is_err());
    }
}
