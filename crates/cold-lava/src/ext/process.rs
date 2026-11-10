//! An extension in a process of its own, behind a `Transport`. It implements `Extension` like a
//! built-in one: host → extension calls go out as JSON-RPC requests and notifications, and what
//! the extension asks comes in on the transport's threads and is answered in `poll`, once a frame,
//! through the `Host` it is given, so its manifest's permissions and the queued effects hold.
//! Nothing here waits for the extension.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use super::installed::Installed;
use super::rpc::{self, Message, Served};
use super::transport::{self, Connector, Launch, Received, Transport};
use super::view::ViewNews;
use super::{
    CommandRequest, EditorRequest, Extension, FileNews, FilePreview, Host, Manifest, PaneNews,
};
use crate::gateway::{self, Edited, News, SubId, Ticket};
use crate::owner::cache::FileRead;
use crate::owner::edits::{FileEdit, NewText};

/// At most this many messages are read from one extension per frame; the rest wait for the next.
const PER_POLL: usize = 64;

/// How long cold-lava waits for an answer before saying so.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    /// For `initialize`; an extension that doesn't answer in time is stopped.
    pub start: Duration,
    /// For `command/run`; the status line says it didn't answer, and it keeps running.
    pub request: Duration,
    /// For `shutdown`, when the workspace closes or cold-lava quits; then it is killed.
    pub shutdown: Duration,
}

pub const TIMEOUTS: Timeouts = Timeouts {
    start: Duration::from_secs(10),
    request: Duration::from_secs(10),
    shutdown: Duration::from_millis(500),
};

pub struct Process {
    manifest: Manifest,
    launch: Launch,
    connector: &'static dyn Connector,
    timeouts: Timeouts,
    state: State,
}

enum State {
    /// No workspace is open, so it isn't started.
    Idle,
    Live(Live),
    /// It ended or was stopped, and why. The status line has said so.
    Stopped(String),
}

struct Live {
    transport: Box<dyn Transport>,
    next_id: u64,
    /// What it hasn't answered yet, by request id.
    waiting: HashMap<u64, (Asked, Instant)>,
    /// Subscriptions it made, stopped when it stops.
    subscriptions: Vec<SubId>,
    /// Its `items/edit` requests answered later, by ticket: their request ids.
    later: HashMap<Ticket, Value>,
}

#[derive(Debug)]
enum Asked {
    Initialize,
    Command(String),
    /// `file/read` for the file at `path`, at version `rev`.
    Read {
        path: String,
        rev: String,
    },
    /// `file/edit` for the file at `path`, at version `rev`.
    Edit {
        path: String,
        rev: String,
    },
    /// `file/preview` for the file at `path`, at version `rev`.
    Preview {
        path: String,
        rev: String,
    },
}

impl Process {
    pub fn new(installed: Installed) -> Option<Process> {
        let connector = transport::connector(&installed.launch.transport)?;
        Some(Process::with(installed, connector, TIMEOUTS))
    }

    pub fn with(installed: Installed, connector: &'static dyn Connector, t: Timeouts) -> Process {
        Process {
            manifest: installed.manifest,
            launch: installed.launch,
            connector,
            timeouts: t,
            state: State::Idle,
        }
    }

    fn start(&mut self, root: &Path, host: &mut Host<'_>) {
        let transport = match self.connector.connect(&self.launch) {
            Ok(t) => t,
            Err(e) => {
                let why = format!("couldn't start: {e}");
                host.notify(format!("{} {why}", self.manifest.name));
                self.state = State::Stopped(why);
                return;
            }
        };
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned());
        let params = json!({
            "apiVersion": rpc::API_VERSION,
            "name": self.manifest.name,
            "workspace": {"root": root, "name": name},
            "permissions": self.manifest.permissions,
            "capabilities": {"methods": rpc::HOST_METHODS},
        });
        self.state = State::Live(Live {
            transport,
            next_id: 1,
            waiting: HashMap::new(),
            subscriptions: Vec::new(),
            later: HashMap::new(),
        });
        self.ask(Asked::Initialize, "initialize", params, host);
    }

    /// Send one request, remembering what it asked.
    fn ask(&mut self, asked: Asked, method: &str, params: Value, host: &mut Host<'_>) {
        let State::Live(live) = &mut self.state else {
            return;
        };
        let id = live.next_id;
        live.next_id += 1;
        let sent = live.transport.send(rpc::request(id, method, params));
        live.waiting.insert(id, (asked, Instant::now()));
        if let Err(e) = sent {
            self.fail(host, e.to_string());
        }
    }

    fn tell(&mut self, message: Vec<u8>, host: &mut Host<'_>) {
        let State::Live(live) = &mut self.state else {
            return;
        };
        if let Err(e) = live.transport.send(message) {
            self.fail(host, e.to_string());
        }
    }

    /// It can't go on: stop it, say why once, with the last line it wrote to its log.
    fn fail(&mut self, host: &mut Host<'_>, why: String) {
        let State::Live(mut live) = std::mem::replace(&mut self.state, State::Idle) else {
            return;
        };
        live.transport.close(Duration::ZERO);
        for sub in live.subscriptions {
            let _ = host.unsubscribe_items(sub);
        }
        for (asked, _) in live.waiting.into_values() {
            match asked {
                Asked::Read { path, rev } => {
                    host.file_given(&path, &rev, Err(format!("it stopped: {why}")))
                }
                Asked::Edit { path, rev } => {
                    host.file_edited(&path, &rev, Err(format!("it stopped: {why}")))
                }
                Asked::Preview { path, rev } => {
                    host.file_previewed(&path, &rev, Err(format!("it stopped: {why}")))
                }
                _ => {}
            }
        }
        let last = (self.launch.log.as_ref())
            .and_then(|log| std::fs::read_to_string(log).ok())
            .and_then(|text| {
                let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
                Some(line.trim().to_string())
            });
        let why = match last {
            Some(last) => format!("{why}: {last}"),
            None => why,
        };
        host.status(None);
        host.notify(format!("{} stopped: {why}", self.manifest.name));
        self.state = State::Stopped(why);
    }

    fn answered(&mut self, id: Value, outcome: Result<Value, rpc::RpcError>, host: &mut Host<'_>) {
        let State::Live(live) = &mut self.state else {
            return;
        };
        let Some((asked, _)) = id.as_u64().and_then(|id| live.waiting.remove(&id)) else {
            return;
        };
        match (asked, outcome) {
            (Asked::Initialize, Err(e)) => {
                self.fail(host, format!("it refused initialize: {}", e.message))
            }
            (Asked::Command(_), Err(e)) => {
                host.notify(format!("{}: {}", self.manifest.name, e.message))
            }
            (Asked::Read { path, rev }, outcome) => {
                let given = outcome.map_err(|e| e.message).and_then(|v| {
                    serde_json::from_value(v).map_err(|e| format!("its answer doesn't read: {e}"))
                });
                host.file_given(&path, &rev, given);
            }
            (Asked::Edit { path, rev }, outcome) => {
                let text = outcome.map_err(|e| e.message).and_then(|v| {
                    serde_json::from_value::<NewText>(v)
                        .map(|t| t.text)
                        .map_err(|e| format!("its answer doesn't read: {e}"))
                });
                host.file_edited(&path, &rev, text);
            }
            (Asked::Preview { path, rev }, outcome) => {
                let content = outcome.map_err(|e| e.message).and_then(|v| {
                    serde_json::from_value(v).map_err(|e| format!("its answer doesn't read: {e}"))
                });
                host.file_previewed(&path, &rev, content);
            }
            _ => {}
        }
    }

    /// Say so for what it didn't answer in time.
    fn check_time(&mut self, host: &mut Host<'_>) {
        let State::Live(live) = &mut self.state else {
            return;
        };
        let t = self.timeouts;
        let late: Vec<u64> = (live.waiting.iter())
            .filter(|(_, (asked, sent))| {
                let limit = match asked {
                    Asked::Initialize => t.start,
                    Asked::Command(_)
                    | Asked::Read { .. }
                    | Asked::Edit { .. }
                    | Asked::Preview { .. } => t.request,
                };
                sent.elapsed() > limit
            })
            .map(|(id, _)| *id)
            .collect();
        let name = &self.manifest.name;
        for id in late {
            let Some((asked, _)) = live.waiting.remove(&id) else {
                continue;
            };
            match asked {
                Asked::Initialize => {
                    let secs = t.start.as_secs_f32();
                    return self.fail(host, format!("it didn't answer initialize in {secs} s"));
                }
                Asked::Command(command) => {
                    let secs = t.request.as_secs_f32();
                    host.notify(format!("{name} didn't answer {command} in {secs} s"));
                }
                Asked::Read { path, rev } => {
                    let secs = t.request.as_secs_f32();
                    let why = format!("it didn't answer file/read in {secs} s");
                    host.file_given(&path, &rev, Err(why));
                }
                Asked::Edit { path, rev } => {
                    let secs = t.request.as_secs_f32();
                    let why = format!("it didn't answer file/edit in {secs} s");
                    host.file_edited(&path, &rev, Err(why));
                }
                Asked::Preview { path, rev } => {
                    let secs = t.request.as_secs_f32();
                    let why = format!("it didn't answer file/preview in {secs} s");
                    host.file_previewed(&path, &rev, Err(why));
                }
            }
        }
    }
}

impl Extension for Process {
    fn initialize(&mut self) -> Manifest {
        self.manifest.clone()
    }

    fn pane(&mut self, news: PaneNews, host: &mut Host<'_>) {
        self.tell(rpc::notification(news), host);
    }

    fn editor(&mut self, request: EditorRequest, host: &mut Host<'_>) {
        self.tell(rpc::notification(request), host);
    }

    fn command(&mut self, request: CommandRequest, host: &mut Host<'_>) {
        let CommandRequest::Run { id, args } = request;
        if !self.running() {
            let name = &self.manifest.name;
            return host.notify(format!("{name} isn't running, so {id} can't run"));
        }
        let params = json!({"id": id, "args": args});
        self.ask(Asked::Command(id), "command/run", params, host);
    }

    fn items(&mut self, news: News, host: &mut Host<'_>) {
        self.tell(rpc::notification(news), host);
    }

    fn view(&mut self, news: ViewNews, host: &mut Host<'_>) {
        self.tell(rpc::notification(news), host);
    }

    fn file(&mut self, news: FileNews, host: &mut Host<'_>) {
        if let FileNews::Open { path, .. } = &news
            && !self.running()
        {
            let name = &self.manifest.name;
            return host.notify(format!("{name} isn't running, so it can't open {path}"));
        }
        self.tell(rpc::notification(news), host);
    }

    fn read_file(&mut self, read: FileRead, host: &mut Host<'_>) {
        if !self.running() {
            let why = format!("{} isn't running", self.manifest.name);
            return host.file_given(&read.path, &read.rev, Err(why));
        }
        let asked = Asked::Read {
            path: read.path.clone(),
            rev: read.rev.clone(),
        };
        let params = serde_json::to_value(read).unwrap_or_default();
        self.ask(asked, "file/read", params, host);
    }

    fn preview_file(&mut self, preview: FilePreview, host: &mut Host<'_>) {
        if !self.running() {
            let why = format!("{} isn't running", self.manifest.name);
            return host.file_previewed(&preview.path, &preview.rev, Err(why));
        }
        let asked = Asked::Preview {
            path: preview.path.clone(),
            rev: preview.rev.clone(),
        };
        let params = serde_json::to_value(preview).unwrap_or_default();
        self.ask(asked, "file/preview", params, host);
    }

    fn edit_file(&mut self, edit: FileEdit, host: &mut Host<'_>) {
        if !self.running() {
            let why = format!("{} isn't running", self.manifest.name);
            return host.file_edited(&edit.path, &edit.rev, Err(why));
        }
        let asked = Asked::Edit {
            path: edit.path.clone(),
            rev: edit.rev.clone(),
        };
        let params = serde_json::to_value(edit).unwrap_or_default();
        self.ask(asked, "file/edit", params, host);
    }

    fn edited(
        &mut self,
        ticket: Ticket,
        answer: Result<Edited, gateway::Error>,
        host: &mut Host<'_>,
    ) {
        let State::Live(live) = &mut self.state else {
            return;
        };
        if let Some(id) = live.later.remove(&ticket) {
            self.tell(rpc::response(id, rpc::answer(answer)), host);
        }
    }

    fn workspace(&mut self, root: Option<&Path>, host: &mut Host<'_>) {
        self.shutdown();
        if let Some(root) = root {
            self.start(root, host);
        }
    }

    fn poll(&mut self, host: &mut Host<'_>) {
        for _ in 0..PER_POLL {
            let State::Live(live) = &mut self.state else {
                return;
            };
            let message = match live.transport.try_recv() {
                Received::Nothing => break,
                Received::Closed(why) => return self.fail(host, why),
                Received::Message(m) => m,
            };
            match rpc::parse(&message) {
                Message::Request { id, method, params } => {
                    let sub = |v: Option<&Value>| v?.get("subscription")?.as_u64().map(SubId);
                    let dropped = sub(params.as_ref());
                    let answer = match rpc::serve(&method, params, host) {
                        Served::Now(answer) => answer,
                        Served::Later(ticket) => {
                            live.later.insert(ticket, id);
                            continue;
                        }
                    };
                    match (&answer, method.as_str()) {
                        (Ok(made), "items/subscribe") => live.subscriptions.extend(sub(Some(made))),
                        (Ok(_), "items/unsubscribe") => {
                            live.subscriptions.retain(|s| Some(*s) != dropped)
                        }
                        _ => {}
                    }
                    self.tell(rpc::response(id, answer), host);
                }
                Message::Notification { method, params } => {
                    let _ = rpc::serve(&method, params, host);
                }
                Message::Response { id, outcome } => self.answered(id, outcome, host),
                Message::Invalid { id, error } => self.tell(rpc::response(id, Err(error)), host),
            }
        }
        self.check_time(host);
    }

    fn running(&self) -> bool {
        matches!(self.state, State::Live(_))
    }

    fn stopped(&self) -> Option<&str> {
        match &self.state {
            State::Stopped(why) => Some(why),
            _ => None,
        }
    }

    /// `shutdown`, then `exit` and the end of its input, waiting a moment for it to go.
    fn shutdown(&mut self) {
        let State::Live(mut live) = std::mem::replace(&mut self.state, State::Idle) else {
            return;
        };
        let id = live.next_id;
        let asked = live
            .transport
            .send(rpc::request(id, "shutdown", Value::Null));
        let until = Instant::now() + self.timeouts.shutdown;
        while asked.is_ok() && Instant::now() < until {
            match live.transport.try_recv() {
                Received::Message(m) => {
                    if let Message::Response { id: answered, .. } = rpc::parse(&m)
                        && answered.as_u64() == Some(id)
                    {
                        break;
                    }
                }
                Received::Nothing => std::thread::sleep(Duration::from_millis(5)),
                Received::Closed(_) => break,
            }
        }
        let exit = json!({"jsonrpc": "2.0", "method": "exit"});
        let _ = live.transport.send(exit.to_string().into_bytes());
        live.transport.close(self.timeouts.shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Core;
    use crate::ext::view::Content;
    use crate::ext::{Effect, ExtId, Permission, Registry};
    use crate::gateway::{Answer, Edit, Filter, Op, Query, Select};
    use std::collections::VecDeque;
    use std::path::PathBuf;

    /// A transport that answers each message cold-lava sends with what `reply` says, or ends
    /// the connection when it says `None`.
    struct Scripted(fn(&Value) -> Option<Vec<Value>>);

    struct Fake {
        reply: fn(&Value) -> Option<Vec<Value>>,
        inbox: VecDeque<Vec<u8>>,
        closed: bool,
    }

    impl Connector for Scripted {
        fn kind(&self) -> &'static str {
            "scripted"
        }
        fn connect(&self, _: &Launch) -> std::io::Result<Box<dyn Transport>> {
            Ok(Box::new(Fake {
                reply: self.0,
                inbox: VecDeque::new(),
                closed: false,
            }))
        }
    }

    impl Transport for Fake {
        fn send(&mut self, message: Vec<u8>) -> std::io::Result<()> {
            let v: Value = serde_json::from_slice(&message).unwrap();
            match (self.reply)(&v) {
                Some(replies) => (self.inbox).extend(replies.iter().map(|r| r.to_string().into())),
                None => self.closed = true,
            }
            Ok(())
        }
        fn try_recv(&mut self) -> Received {
            match self.inbox.pop_front() {
                Some(m) => Received::Message(m),
                None if self.closed => Received::Closed("it exited with status 1".into()),
                None => Received::Nothing,
            }
        }
        fn close(&mut self, _: Duration) {
            self.closed = true;
        }
    }

    fn installed(dir: PathBuf, log: Option<PathBuf>) -> Installed {
        let manifest = Manifest {
            name: "fake".into(),
            title: "fake".into(),
            version: String::new(),
            description: String::new(),
            roles: Vec::new(),
            hints: Vec::new(),
            commands: Vec::new(),
            types: Default::default(),
            shapes: Vec::new(),
            permissions: crate::ext::read_only(),
            file_types: Vec::new(),
            shows: Vec::new(),
        };
        let launch = Launch {
            dir,
            transport: "scripted".into(),
            run: vec!["fake".into()],
            log,
        };
        Installed { manifest, launch }
    }

    const QUICK: Timeouts = Timeouts {
        start: Duration::from_millis(100),
        request: Duration::from_millis(100),
        shutdown: Duration::from_millis(100),
    };

    fn workspace(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-process-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(dir.join("Plan.md"), "- [ ] Pay rent\n").unwrap();
        dir
    }

    fn scripted(reply: &'static Scripted, dir: &Path, log: Option<PathBuf>) -> Registry {
        let p = Process::with(installed(dir.into(), log), reply, QUICK);
        Registry::new(vec![Box::new(p)])
    }

    fn answer(request: &Value, result: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": request["id"], "result": result})
    }

    fn run(id: &str) -> CommandRequest {
        CommandRequest::Run {
            id: id.into(),
            args: Value::Null,
        }
    }

    fn notes(effects: &[Effect]) -> Vec<&str> {
        (effects.iter())
            .filter_map(|e| match e {
                Effect::Notify { message } => Some(message.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Poll for `wait`, and what the extension asked for meanwhile.
    fn poll_for(r: &mut Registry, core: &mut Core, wait: Duration) -> Vec<Effect> {
        let (mut effects, until) = (Vec::new(), Instant::now() + wait);
        while Instant::now() < until {
            effects.extend(r.call(ExtId(0), Some(core), |e, h| e.poll(h)));
            std::thread::sleep(Duration::from_millis(5));
        }
        effects
    }

    fn tick_first(core: &mut Core, select: Select) {
        let query = Query {
            select,
            ..Default::default()
        };
        let task = core.query_items(&query).unwrap().items.remove(0);
        let tick = Edit {
            id: task.id,
            rev: task.rev,
            op: Op::Toggle,
        };
        core.edit_item("test", &tick).unwrap();
    }

    fn tasks(status: &[&str]) -> Select {
        let status = status.iter().map(|s| s.to_string()).collect();
        Select {
            kind: "task".into(),
            filter: Filter {
                status,
                ..Default::default()
            },
        }
    }

    #[test]
    fn an_extension_that_doesnt_answer_initialize_is_stopped_once() {
        static HUNG: Scripted = Scripted(|_| Some(Vec::new()));
        let dir = workspace("hung");
        let mut core = Core::open(&dir).unwrap();
        let mut r = scripted(&HUNG, &dir, None);
        r.call(ExtId(0), Some(&mut core), |e, h| e.workspace(Some(&dir), h));
        assert!(r.running(ExtId(0)));
        let effects = poll_for(&mut r, &mut core, Duration::from_millis(300));
        let why = "it didn't answer initialize in 0.1 s";
        assert_eq!(notes(&effects), [format!("fake stopped: {why}")]);
        assert!(!r.running(ExtId(0)), "its commands can't run");
        assert_eq!(r.stopped(ExtId(0)), Some(why));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_preview_comes_back_from_its_owner_and_one_not_answered_or_too_big_fails() {
        static PREVIEWS: Scripted = Scripted(|m| {
            let row = |i: usize| json!({"id": i.to_string(), "cells": [i.to_string()]});
            let table = |n: usize| {
                json!({"kind": "table", "title": "Data.csv",
                       "columns": [{"name": "n", "type": "text"}],
                       "rows": (0..n).map(row).collect::<Vec<_>>()})
            };
            Some(match (m["method"].as_str(), m["params"]["path"].as_str()) {
                (Some("initialize"), _) => vec![answer(m, json!({}))],
                (Some("file/preview"), Some("Data.csv")) => {
                    assert_eq!(
                        (
                            m["params"]["width"].as_u64(),
                            m["params"]["height"].as_u64()
                        ),
                        (Some(40), Some(8))
                    );
                    vec![answer(m, table(2))]
                }
                (Some("file/preview"), Some("Big.csv")) => vec![answer(m, table(201))],
                _ => Vec::new(),
            })
        });
        let dir = workspace("previews");
        let mut core = Core::open(&dir).unwrap();
        let mut installed = installed(dir.clone(), None);
        installed.manifest.file_types = vec![crate::ext::FileType {
            extension: "csv".into(),
            title: "CSV".into(),
            new: None,
        }];
        let p = Process::with(installed, &PREVIEWS, QUICK);
        let mut r = Registry::new(vec![Box::new(p)]);
        r.call(ExtId(0), Some(&mut core), |e, h| e.workspace(Some(&dir), h));
        for path in ["Data.csv", "Big.csv", "Slow.csv"] {
            let ask = FilePreview {
                path: path.into(),
                rev: "r1".into(),
                width: 40,
                height: 8,
            };
            r.call(ExtId(0), Some(&mut core), |e, h| e.preview_file(ask, h));
        }
        let effects = poll_for(&mut r, &mut core, Duration::from_millis(300));
        let previewed: Vec<(&str, Result<usize, &str>)> = (effects.iter())
            .filter_map(|e| match e {
                Effect::Previewed { path, content, .. } => Some((
                    path.as_str(),
                    match content {
                        Ok(Content::Table(t)) => Ok(t.rows.len()),
                        Ok(_) => Ok(0),
                        Err(why) => Err(why.as_str()),
                    },
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            previewed,
            [
                ("Data.csv", Ok(2)),
                (
                    "Big.csv",
                    Err("a preview has at most 200 rows and groups; this one has 201")
                ),
                ("Slow.csv", Err("it didn't answer file/preview in 0.1 s")),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_command_that_isnt_answered_is_reported_and_the_extension_keeps_running() {
        static SLOW: Scripted = Scripted(|m| {
            Some(match (m["method"].as_str(), m["params"]["id"].as_str()) {
                (Some("initialize"), _) => vec![answer(m, json!({}))],
                (Some("command/run"), Some("fake.bad")) => {
                    let error = json!({"code": -32603, "message": "no such thing"});
                    vec![json!({"jsonrpc": "2.0", "id": m["id"], "error": error})]
                }
                _ => Vec::new(),
            })
        });
        let dir = workspace("slow");
        let mut core = Core::open(&dir).unwrap();
        let mut r = scripted(&SLOW, &dir, None);
        r.call(ExtId(0), Some(&mut core), |e, h| e.workspace(Some(&dir), h));
        r.call(ExtId(0), Some(&mut core), |e, h| {
            e.command(run("fake.go"), h)
        });
        r.call(ExtId(0), Some(&mut core), |e, h| {
            e.command(run("fake.bad"), h)
        });
        let effects = poll_for(&mut r, &mut core, Duration::from_millis(300));
        assert_eq!(
            notes(&effects),
            ["fake: no such thing", "fake didn't answer fake.go in 0.1 s"]
        );
        assert!(r.running(ExtId(0)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_is_reported_once_with_its_log_and_its_subscriptions_stop() {
        static CRASH: Scripted = Scripted(|m| match m["method"].as_str() {
            Some("initialize") => Some(vec![
                answer(m, json!({})),
                json!({"jsonrpc": "2.0", "id": 1, "method": "items/subscribe",
                       "params": {"kind": "task"}}),
                json!({"jsonrpc": "2.0", "method": "ui/status", "params": {"text": "1 open"}}),
            ]),
            Some("command/run") => None,
            _ => Some(Vec::new()),
        });
        let dir = workspace("crash");
        let log = dir.join("fake.log");
        std::fs::write(&log, "starting\nTraceback: ZeroDivisionError\n\n").unwrap();
        let mut core = Core::open(&dir).unwrap();
        let mut r = scripted(&CRASH, &dir, Some(log));
        r.call(ExtId(0), Some(&mut core), |e, h| e.workspace(Some(&dir), h));
        let effects = poll_for(&mut r, &mut core, Duration::from_millis(50));
        let shown = Effect::Status {
            text: Some("1 open".into()),
        };
        assert_eq!(effects, [shown]);

        r.call(ExtId(0), Some(&mut core), |e, h| {
            e.command(run("fake.go"), h)
        });
        let effects = poll_for(&mut r, &mut core, Duration::from_millis(50));
        let message = "fake stopped: it exited with status 1: Traceback: ZeroDivisionError";
        let stopped = Effect::Notify {
            message: message.into(),
        };
        assert_eq!(effects, [Effect::Status { text: None }, stopped]);
        assert!(!r.running(ExtId(0)));

        tick_first(&mut core, tasks(&[]));
        assert_eq!(core.take_item_news(), [], "its subscription went with it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_edit_of_an_owned_file_is_answered_once_its_owner_wrote_it() {
        use crate::ext::FileType;
        static OWNER: Scripted = Scripted(|m| {
            let (method, params) = (m["method"].as_str(), &m["params"]);
            let given = |vei: u32| json!({"items": [{"place": "1", "type": "eruption", "fields": {"vei": vei}}]});
            Some(match method {
                Some("initialize") => vec![answer(m, json!({}))],
                Some("file/read") => {
                    let vei = params["text"].as_str().unwrap().trim();
                    vec![answer(m, given(vei.parse().unwrap()))]
                }
                // 8 is never answered.
                Some("file/edit") => match params["changes"][0]["value"].as_u64() {
                    Some(7) => vec![answer(m, json!({"text": "7\n"}))],
                    _ => Vec::new(),
                },
                Some("command/run") => vec![
                    answer(m, Value::Null),
                    json!({"jsonrpc": "2.0", "id": 100, "method": "items/query",
                           "params": {"kind": "eruption"}}),
                ],
                None if m["id"] == 100 => {
                    let item = &m["result"]["items"][0];
                    let edit = json!({"id": item["id"], "rev": item["rev"], "op": "set-field",
                                      "field": "vei", "value": 7});
                    vec![json!({"jsonrpc": "2.0", "id": 101, "method": "items/edit",
                                "params": edit})]
                }
                None if m["id"] == 101 => {
                    let said = format!("vei {}", m["result"]["item"]["vei"]);
                    vec![json!({"jsonrpc": "2.0", "method": "ui/notify",
                                "params": {"message": said}})]
                }
                _ => Vec::new(),
            })
        });
        let dir = workspace("owner-edit");
        std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
        let types = "version = 1\n[types.eruption]\non = \"table-row\"\nwhere.header = [\"VEI\"]\n\
                     fields.vei = { column = \"VEI\", type = \"number\" }\n";
        std::fs::write(dir.join(".cold-lava/types.toml"), types).unwrap();
        std::fs::write(dir.join("Eruptions.csv"), "6\n").unwrap();
        let mut owner = installed(dir.clone(), None);
        owner.manifest.permissions = vec![Permission::ItemsRead, Permission::ItemsEdit];
        owner.manifest.file_types = vec![FileType {
            extension: "csv".into(),
            title: "CSV".into(),
            new: None,
        }];
        let p = Process::with(owner, &OWNER, QUICK);
        let mut r = Registry::new(vec![Box::new(p)]);
        let mut core = Core::open(&dir).unwrap();
        r.call(ExtId(0), Some(&mut core), |e, h| e.workspace(Some(&dir), h));
        core.set_owners(r.owned_kinds(), None);
        // What the menu does each frame: poll, ask the owner, hand out the answers.
        let frames = |r: &mut Registry, core: &mut Core, n: usize| {
            let mut effects = Vec::new();
            for _ in 0..n {
                effects.extend(r.call(ExtId(0), Some(core), |e, h| e.poll(h)));
                for (_, read) in core.take_reads() {
                    effects.extend(r.call(ExtId(0), Some(core), |e, h| e.read_file(read, h)));
                }
                for (_, edit) in core.take_file_edits() {
                    effects.extend(r.call(ExtId(0), Some(core), |e, h| e.edit_file(edit, h)));
                }
                for (_, ticket, answer) in core.take_edited() {
                    let f = |e: &mut dyn Extension, h: &mut Host<'_>| e.edited(ticket, answer, h);
                    effects.extend(r.call(ExtId(0), Some(core), f));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            effects
        };
        frames(&mut r, &mut core, 5);
        r.call(ExtId(0), Some(&mut core), |e, h| {
            e.command(run("fake.go"), h)
        });
        let effects = frames(&mut r, &mut core, 20);
        assert_eq!(notes(&effects), ["vei 7"], "the answer once it is written");
        assert_eq!(
            std::fs::read_to_string(dir.join("Eruptions.csv")).unwrap(),
            "7\n"
        );

        let query = Query {
            select: Select {
                kind: "eruption".into(),
                ..Select::default()
            },
            ..Query::default()
        };
        let row = core.query_items(&query).unwrap().items.remove(0);
        let set = Edit {
            id: row.id,
            rev: row.rev,
            op: Op::SetField {
                field: "vei".into(),
                value: json!(8),
            },
        };
        assert!(matches!(core.edit_item("test", &set), Ok(Answer::Later(_))));
        frames(&mut r, &mut core, 2);
        std::thread::sleep(Duration::from_millis(150));
        r.call(ExtId(0), Some(&mut core), |e, h| e.poll(h));
        let answered = core.take_edited();
        let [(by, _, Err(e))] = &answered[..] else {
            panic!("{answered:?}");
        };
        assert_eq!(by, "test");
        assert_eq!(
            e.message,
            "fake didn't edit Eruptions.csv: it didn't answer file/edit in 0.1 s"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("Eruptions.csv")).unwrap(),
            "7\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let dest = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &dest);
            } else {
                std::fs::copy(entry.path(), dest).unwrap();
            }
        }
    }

    /// Poll, handing the core's item news to the extension as the menu does, until an effect
    /// passes `done`, which it returns.
    fn until(r: &mut Registry, core: &mut Core, done: impl Fn(&Effect) -> bool) -> Effect {
        let (mut effects, deadline) = (Vec::new(), Instant::now() + Duration::from_secs(20));
        while Instant::now() < deadline {
            core.poll();
            for (_, news) in core.take_item_news() {
                effects.extend(r.call(ExtId(0), Some(core), |e, h| e.items(news, h)));
            }
            effects.extend(r.call(ExtId(0), Some(core), |e, h| e.poll(h)));
            if let Some(found) = effects.iter().find(|e| done(e)) {
                return found.clone();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out; heard {effects:?}");
    }

    fn status(text: String) -> impl Fn(&Effect) -> bool {
        move |e| matches!(e, Effect::Status { text: Some(t) } if *t == text)
    }

    fn has_python() -> bool {
        let python = std::process::Command::new("python3")
            .arg("--version")
            .output();
        let found = python.is_ok_and(|o| o.status.success());
        if !found {
            eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        }
        found
    }

    /// An example extension (`examples/extensions/<example>`), loaded from a copy beside a copy
    /// of the example workspace with an overdue task in Inbox/Seismograph.md: its registry, the
    /// core, the workspace and the folder that holds both. Its log is `<example>.log` there.
    fn python_example(example: &str, name: &str) -> (Registry, Core, PathBuf, PathBuf) {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = std::env::temp_dir().join(format!("cold-lava-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (root, extensions) = (dir.join("workspace"), dir.join("extensions"));
        copy_dir(&repo.join("example-workspace"), &root);
        let late = "- [ ] Check the seismograph 📅 2000-01-01\n";
        std::fs::write(root.join("Inbox/Seismograph.md"), late).unwrap();
        let from = repo.join("examples/extensions").join(example);
        copy_dir(&from, &extensions.join(example));
        let (found, mistakes) = crate::ext::installed::load(&extensions);
        assert_eq!(mistakes, Vec::<String>::new());
        let mut installed = found.into_iter().next().unwrap();
        installed.launch.log = Some(dir.join(format!("{example}.log")));
        let r = Registry::new(vec![Box::new(Process::new(installed).unwrap())]);
        let core = Core::open(&root).unwrap();
        (r, core, root, dir)
    }

    #[test]
    fn the_examples_share_one_helper() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions");
        let helper =
            |example: &str| std::fs::read_to_string(examples.join(example).join("cold_lava.py"));
        let first = helper("open-tasks").unwrap();
        let mut seen = 0;
        for example in std::fs::read_dir(&examples).unwrap().flatten() {
            let name = example.file_name().to_string_lossy().into_owned();
            assert_eq!(
                helper(&name).unwrap(),
                first,
                "{name}: each example carries a copy of cold_lava.py, so it can be copied \
                 alone: keep them alike"
            );
            seen += 1;
        }
        assert!(seen >= 3, "every example is checked");
    }

    #[test]
    fn the_csv_example_says_what_its_files_say_as_items_and_links() {
        use crate::core::Event;
        use crate::gateway::Page;
        use crate::owner::cache::Kind;
        if !has_python() {
            return;
        }
        let (mut r, mut core, root, dir) = python_example("csv-table", "python-csv-read");
        r.call(ExtId(0), Some(&mut core), |e, h| {
            e.workspace(Some(&root), h)
        });
        core.set_owners(r.owned_kinds(), None);
        assert_eq!(
            r.owned_kinds(),
            [Kind {
                extension: "csv".into(),
                owner: "csv-table".into(),
                version: "0.1.0".into(),
            }]
        );
        let reads = core.take_reads();
        assert_eq!(
            reads.len(),
            2,
            "Eruptions.csv and Observatories.csv: {reads:?}"
        );
        for (_, read) in reads {
            r.call(ExtId(0), Some(&mut core), |e, h| e.read_file(read, h));
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while (core.index.workspace.owned.iter()).any(|f| f.rev.is_none()) {
            assert!(
                Instant::now() < deadline,
                "csv-table didn't answer file/read"
            );
            r.call(ExtId(0), Some(&mut core), |e, h| e.poll(h));
            std::thread::sleep(Duration::from_millis(10));
        }
        let events = core.poll();
        assert!(events.contains(&Event::OwnedChanged("Volcanoes/Eruptions.csv".into())));
        let found = |core: &Core, kind: &str| {
            let select = Select {
                kind: kind.into(),
                filter: Filter {
                    folder: Some("Volcanoes".into()),
                    ..Filter::default()
                },
            };
            let query = Query {
                select,
                page: Page::default(),
            };
            core.query_items(&query).unwrap().items
        };
        let tasks = found(&core, "task");
        assert_eq!(tasks.len(), 1);
        let places = &core.index.workspace.owned[0].places;
        let place = places.get(&tasks[0].id).map(String::as_str);
        assert_eq!(place, Some("[[Krakatoa]]#Notes"));
        let rows = found(&core, "eruption");
        let volcanoes: Vec<String> = (rows.iter())
            .map(|i| serde_json::to_value(i).unwrap()["volcanoes"][0].to_string())
            .collect();
        assert_eq!(
            volcanoes,
            [
                r#""Vesuvius""#,
                r#""Krakatoa""#,
                r#""Mount St. Helens""#,
                r#""Eyjafjallajökull""#
            ]
        );
        let krakatoa = core.index.by_rel("Volcanoes/Krakatoa").unwrap();
        assert_eq!(core.index.backlinks(krakatoa).len(), 5, "two from the CSV");
        assert_eq!(
            std::fs::read_to_string(dir.join("csv-table.log")).unwrap(),
            ""
        );

        r.call(ExtId(0), Some(&mut core), |e, h| e.workspace(None, h));
        let read = crate::owner::cache::FileRead {
            path: "Volcanoes/Eruptions.csv".into(),
            rev: "0".into(),
            text: String::new(),
        };
        r.call(ExtId(0), Some(&mut core), |e, h| e.read_file(read, h));
        let trouble = core.poll().into_iter().find_map(|e| match e {
            Event::OwnerTrouble { message, .. } => Some(message),
            _ => None,
        });
        assert_eq!(trouble.as_deref(), Some("csv-table isn't running"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_csv_example_saves_on_top_of_a_change_it_hadnt_heard_of() {
        use crate::ext::FileNews;
        use crate::ext::view::{Event, Happened, ViewNews};
        if !has_python() {
            return;
        }
        let (mut r, mut core, root, dir) = python_example("csv-table", "python-csv");
        r.call(ExtId(0), Some(&mut core), |e, h| {
            e.workspace(Some(&root), h)
        });
        let path = "Volcanoes/Eruptions.csv";
        let open = FileNews::Open {
            path: path.into(),
            place: None,
        };
        r.call(ExtId(0), Some(&mut core), |e, h| e.file(open, h));
        let opened = until(&mut r, &mut core, |e| matches!(e, Effect::OpenView { .. }));
        let Effect::OpenView { view, .. } = opened else {
            unreachable!()
        };
        let elsewhere = "Volcano,Year,VEI,Notes\nTambora,1815,7,\nKrakatoa,1883,6,\n";
        std::fs::write(root.join(path), elsewhere).unwrap();
        let event = Event::Edit {
            row: "2".into(),
            column: "VEI".into(),
            value: "6.5".into(),
        };
        let edit = ViewNews::Event(Happened { view, event });
        r.call(ExtId(0), Some(&mut core), |e, h| e.view(edit, h));
        let saved = format!("saved {path}");
        until(&mut r, &mut core, |e| {
            notes(std::slice::from_ref(e)) == [saved.as_str()]
        });
        let text = std::fs::read_to_string(root.join(path)).unwrap();
        assert_eq!(
            text, "Volcano,Year,VEI,Notes\nTambora,1815,7,\nKrakatoa,1883,6.5,\n",
            "refused as changed, read again, and the cell set in what was there"
        );
        r.shutdown();
        let log = std::fs::read_to_string(dir.join("csv-table.log")).unwrap();
        assert_eq!(log, "", "nothing on stderr");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_python_example_counts_moves_and_follows_tasks() {
        if !has_python() {
            return;
        }
        let (mut r, mut core, root, dir) = python_example("open-tasks", "python");
        let open = Query {
            select: tasks(&["todo", "in_progress"]),
            ..Default::default()
        };
        let n = core.query_items(&open).unwrap().items.len();

        r.call(ExtId(0), Some(&mut core), |e, h| {
            e.workspace(Some(&root), h)
        });
        until(&mut r, &mut core, status(format!("☐ {n} open tasks")));

        r.call(ExtId(0), Some(&mut core), |e, h| {
            e.command(run("open-tasks.reschedule"), h)
        });
        let moved = "moved 1 overdue task to today";
        until(&mut r, &mut core, |e| {
            notes(std::slice::from_ref(e)) == [moved]
        });
        let text = std::fs::read_to_string(root.join("Inbox/Seismograph.md")).unwrap();
        let today = crate::tasks::today().format("%Y-%m-%d");
        assert_eq!(text, format!("- [ ] Check the seismograph 📅 {today}\n"));

        tick_first(&mut core, tasks(&["todo", "in_progress"]));
        let fewer = format!("☐ {} open tasks", n - 1);
        until(&mut r, &mut core, status(fewer));

        r.shutdown();
        assert!(!r.running(ExtId(0)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_python_example_opens_a_list_and_hears_what_the_user_did_in_it() {
        use crate::ext::view::{Content, Event, Happened, ViewNews};
        if !has_python() {
            return;
        }
        let (mut r, mut core, root, dir) = python_example("open-tasks", "python-view");
        r.call(ExtId(0), Some(&mut core), |e, h| {
            e.workspace(Some(&root), h)
        });
        until(&mut r, &mut core, |e| matches!(e, Effect::Status { .. }));
        r.call(ExtId(0), Some(&mut core), |e, h| {
            e.command(run("open-tasks.list"), h)
        });
        let opened = until(&mut r, &mut core, |e| matches!(e, Effect::OpenView { .. }));
        let Effect::OpenView {
            view,
            content: Content::List(list),
            ..
        } = opened
        else {
            panic!("{opened:?}")
        };
        let group = |title: &str| list.groups.iter().find(|g| g.title == title).cloned();
        let inbox = group("Inbox/Seismograph").expect("a group for each note");
        let late = inbox.rows[0].clone();
        assert_eq!(late.text[0].text, "Check the seismograph");
        assert_eq!(late.right[0].text, "2000-01-01");
        assert_eq!(list.keys[0].command, "today");

        let happened = |event| ViewNews::Event(Happened { view, event });
        let tick = happened(Event::Toggled {
            row: late.id.clone(),
        });
        r.call(ExtId(0), Some(&mut core), |e, h| e.view(tick, h));
        let updated = until(&mut r, &mut core, |e| {
            matches!(e, Effect::UpdateView { .. })
        });
        let text = std::fs::read_to_string(root.join("Inbox/Seismograph.md")).unwrap();
        assert!(text.starts_with("- [x] Check the seismograph"), "{text}");
        let Effect::UpdateView {
            content: Content::List(list),
            ..
        } = updated
        else {
            panic!("{updated:?}")
        };
        assert!(
            list.groups.iter().all(|g| g.title != "Inbox/Seismograph"),
            "a ticked task isn't open"
        );

        let journal = list.groups.iter().find(|g| g.title == "Journal/2026-11-05");
        let first = journal.unwrap().rows[0].id.clone();
        let open = happened(Event::Activated { row: first });
        r.call(ExtId(0), Some(&mut core), |e, h| e.view(open, h));
        let note = until(&mut r, &mut core, |e| matches!(e, Effect::OpenNote { .. }));
        let Effect::OpenNote { note, line, .. } = note else {
            unreachable!()
        };
        assert_eq!(note, Path::new("Journal/2026-11-05.md"));
        assert!(line.is_some());

        let closed = ViewNews::Closed { view };
        r.call(ExtId(0), Some(&mut core), |e, h| e.view(closed, h));
        until(&mut r, &mut core, |e| {
            notes(std::slice::from_ref(e)) == ["closed the list of open tasks"]
        });
        r.shutdown();
        let log = std::fs::read_to_string(dir.join("open-tasks.log")).unwrap();
        assert_eq!(log, "", "nothing on stderr");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
