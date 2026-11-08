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
use super::rpc::{self, Message};
use super::transport::{self, Connector, Launch, Received, Transport};
use super::{CommandRequest, EditorRequest, Extension, Host, Manifest, PaneNews};
use crate::gateway::{News, SubId};

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
}

#[derive(Debug)]
enum Asked {
    Initialize,
    Command(String),
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
                    Asked::Command(_) => t.request,
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
                    let answer = rpc::serve(&method, params, host);
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
    use crate::ext::{Effect, ExtId, Registry};
    use crate::gateway::{Edit, Filter, Op, Query, Select};
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
            permissions: crate::ext::read_only(),
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
    /// passes `done`.
    fn until(r: &mut Registry, core: &mut Core, done: impl Fn(&Effect) -> bool) {
        let (mut effects, deadline) = (Vec::new(), Instant::now() + Duration::from_secs(20));
        while Instant::now() < deadline {
            core.poll();
            for (_, news) in core.take_item_news() {
                effects.extend(r.call(ExtId(0), Some(core), |e, h| e.items(news, h)));
            }
            effects.extend(r.call(ExtId(0), Some(core), |e, h| e.poll(h)));
            if effects.iter().any(&done) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out; heard {effects:?}");
    }

    fn status(text: String) -> impl Fn(&Effect) -> bool {
        move |e| matches!(e, Effect::Status { text: Some(t) } if *t == text)
    }

    #[test]
    fn the_python_example_counts_moves_and_follows_tasks() {
        let python = std::process::Command::new("python3")
            .arg("--version")
            .output();
        if !python.is_ok_and(|o| o.status.success()) {
            eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
            return;
        }
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = std::env::temp_dir().join(format!("cold-lava-python-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (root, extensions) = (dir.join("workspace"), dir.join("extensions"));
        copy_dir(&repo.join("example-workspace"), &root);
        let late = "- [ ] Check the seismograph 📅 2000-01-01\n";
        std::fs::write(root.join("Inbox/Seismograph.md"), late).unwrap();
        let example = repo.join("examples/extensions/open-tasks");
        copy_dir(&example, &extensions.join("open-tasks"));
        let (found, mistakes) = crate::ext::installed::load(&extensions);
        assert_eq!(mistakes, Vec::<String>::new());
        let mut example = found.into_iter().next().unwrap();
        example.launch.log = Some(dir.join("open-tasks.log"));
        let mut r = Registry::new(vec![Box::new(Process::new(example).unwrap())]);
        let mut core = Core::open(&root).unwrap();
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
}
