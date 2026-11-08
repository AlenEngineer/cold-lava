//! How bytes travel between cold-lava and an extension in another process (docs/extensions.md,
//! "Transport, as built"). The protocol above only sees whole messages, and never waits: a
//! transport does its reading and writing on threads of its own, so the menu polls it once a
//! frame and a slow or stuck extension can't hold the screen.

use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::time::{Duration, Instant};

use super::frame;

/// Messages waiting in each direction before the sender has to wait (the extension) or is told
/// the other side isn't reading (cold-lava).
const QUEUE: usize = 256;

/// What came in from the extension.
#[derive(Debug, PartialEq)]
pub enum Received {
    Message(Vec<u8>),
    Nothing,
    /// The connection ended, and why. Nothing more comes after this.
    Closed(String),
}

/// A message-framed connection to one extension. None of these wait for the extension.
pub trait Transport: Send {
    /// Queue one whole message (a serialized JSON-RPC object).
    fn send(&mut self, message: Vec<u8>) -> io::Result<()>;
    /// The next message that came in, if any.
    fn try_recv(&mut self) -> Received;
    /// End the connection: the extension sees its input end, and a process that is still running
    /// after `grace` is killed.
    fn close(&mut self, grace: Duration);
}

/// How to start an extension: what its manifest says, where it was found, where its log goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    /// The folder its manifest is in, where its program runs.
    pub dir: PathBuf,
    pub transport: String,
    /// The program and its arguments, run in `dir`.
    pub run: Vec<String>,
    /// Where what it writes to stderr goes, new on each start.
    pub log: Option<PathBuf>,
}

/// Knows how to open one kind of transport, by the `transport` value in a manifest.
pub trait Connector: Sync {
    fn kind(&self) -> &'static str;
    fn connect(&self, launch: &Launch) -> io::Result<Box<dyn Transport>>;
}

/// Every kind of transport cold-lava has. Adding one is a new `Connector` here and nothing else.
pub static CONNECTORS: &[&dyn Connector] = &[&Stdio];

pub fn connector(kind: &str) -> Option<&'static dyn Connector> {
    CONNECTORS.iter().copied().find(|c| c.kind() == kind)
}

/// Starts the extension's program and talks over its stdin and stdout.
pub struct Stdio;

impl Connector for Stdio {
    fn kind(&self) -> &'static str {
        "stdio"
    }

    fn connect(&self, launch: &Launch) -> io::Result<Box<dyn Transport>> {
        let Some((program, args)) = launch.run.split_first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "nothing to run",
            ));
        };
        let stderr = match &launch.log {
            Some(log) => process::Stdio::from(new_log(log)?),
            None => process::Stdio::null(),
        };
        let mut child = Command::new(program)
            .args(args)
            .current_dir(&launch.dir)
            .stdin(process::Stdio::piped())
            .stdout(process::Stdio::piped())
            .stderr(stderr)
            .spawn()
            .map_err(|e| io::Error::new(e.kind(), format!("couldn't start {program}: {e}")))?;
        let (stdin, stdout) = (child.stdin.take(), child.stdout.take());
        let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
            let _ = child.kill();
            return Err(io::Error::other("no pipes to the program"));
        };

        let (to_ext, outbox) = mpsc::sync_channel::<Vec<u8>>(QUEUE);
        std::thread::spawn(move || {
            let mut w = BufWriter::new(stdin);
            for message in outbox {
                if frame::write(&mut w, &message).is_err() {
                    break;
                }
            }
        });
        let (inbox, from_ext) = mpsc::sync_channel::<Result<Vec<u8>, String>>(QUEUE);
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let next = match frame::read(&mut r) {
                    Ok(Some(message)) => Ok(message),
                    Ok(None) => Err(String::new()),
                    Err(e) => Err(format!("it sent something cold-lava can't read: {e}")),
                };
                let end = next.is_err();
                if inbox.send(next).is_err() || end {
                    break;
                }
            }
        });
        Ok(Box::new(Process {
            child,
            to_ext: Some(to_ext),
            from_ext,
            closed: false,
        }))
    }
}

fn new_log(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    File::create(path)
}

struct Process {
    child: Child,
    to_ext: Option<SyncSender<Vec<u8>>>,
    from_ext: Receiver<Result<Vec<u8>, String>>,
    closed: bool,
}

impl Process {
    /// Its exit status, waiting up to `grace` for it.
    fn wait(&mut self, grace: Duration) -> Option<ExitStatus> {
        let until = Instant::now() + grace;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(5)),
                _ => return None,
            }
        }
    }

    /// Why the connection ended, once its output did.
    fn why(&mut self, read: String) -> String {
        if !read.is_empty() {
            let _ = self.child.kill();
            self.wait(Duration::from_millis(200));
            return read;
        }
        match self.wait(Duration::from_millis(200)) {
            Some(status) if status.success() => "it exited".to_string(),
            Some(status) => match status.code() {
                Some(code) => format!("it exited with status {code}"),
                None => "it was killed".to_string(),
            },
            None => {
                let _ = self.child.kill();
                self.wait(Duration::from_millis(200));
                "it closed its output".to_string()
            }
        }
    }
}

impl Transport for Process {
    fn send(&mut self, message: Vec<u8>) -> io::Result<()> {
        let Some(to_ext) = &self.to_ext else {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "closed"));
        };
        to_ext.try_send(message).map_err(|e| match e {
            TrySendError::Full(_) => io::Error::new(
                io::ErrorKind::WouldBlock,
                "it isn't reading what cold-lava sends",
            ),
            TrySendError::Disconnected(_) => {
                io::Error::new(io::ErrorKind::BrokenPipe, "its input is closed")
            }
        })
    }

    fn try_recv(&mut self) -> Received {
        if self.closed {
            return Received::Closed("closed".into());
        }
        match self.from_ext.try_recv() {
            Ok(Ok(message)) => Received::Message(message),
            Err(TryRecvError::Empty) => Received::Nothing,
            Ok(Err(read)) => {
                self.closed = true;
                Received::Closed(self.why(read))
            }
            Err(TryRecvError::Disconnected) => {
                self.closed = true;
                Received::Closed(self.why(String::new()))
            }
        }
    }

    fn close(&mut self, grace: Duration) {
        self.to_ext = None;
        self.closed = true;
        if self.wait(grace).is_none() {
            let _ = self.child.kill();
            self.wait(Duration::from_millis(200));
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.close(Duration::ZERO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, log: Option<PathBuf>) -> Launch {
        Launch {
            dir: std::env::temp_dir(),
            transport: "stdio".into(),
            run: vec!["sh".into(), "-c".into(), script.into()],
            log,
        }
    }

    fn next(t: &mut dyn Transport) -> Received {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            match t.try_recv() {
                Received::Nothing if Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                r => return r,
            }
        }
    }

    #[test]
    fn stdio_carries_whole_messages_both_ways() {
        let mut t = Stdio.connect(&sh("cat", None)).unwrap();
        assert_eq!(t.try_recv(), Received::Nothing, "never waits");
        t.send(b"{\"a\":1}".to_vec()).unwrap();
        t.send("{\"b\":\"🌋\"}".into()).unwrap();
        assert_eq!(next(t.as_mut()), Received::Message(b"{\"a\":1}".to_vec()));
        assert_eq!(next(t.as_mut()), Received::Message("{\"b\":\"🌋\"}".into()));
        t.close(Duration::from_secs(2));
        assert!(matches!(next(t.as_mut()), Received::Closed(_)));
    }

    #[test]
    fn a_crash_says_how_it_ended_and_stderr_goes_to_the_log() {
        let log = std::env::temp_dir().join(format!("cold-lava-crash-{}.log", std::process::id()));
        let mut t = Stdio
            .connect(&sh("echo boom >&2; exit 3", Some(log.clone())))
            .unwrap();
        assert_eq!(
            next(t.as_mut()),
            Received::Closed("it exited with status 3".into())
        );
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "boom\n");
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn garbage_on_stdout_ends_the_connection() {
        let mut t = Stdio.connect(&sh("echo hello; sleep 30", None)).unwrap();
        let Received::Closed(why) = next(t.as_mut()) else {
            panic!("still open");
        };
        assert!(
            why.contains("can't read") && why.contains("not a header"),
            "{why}"
        );
    }

    #[test]
    fn a_program_that_isnt_there_is_an_error() {
        let launch = Launch {
            run: vec!["cold-lava-no-such-program".into()],
            ..sh("", None)
        };
        let e = Stdio.connect(&launch).err().unwrap().to_string();
        assert!(
            e.contains("couldn't start cold-lava-no-such-program"),
            "{e}"
        );
    }
}
