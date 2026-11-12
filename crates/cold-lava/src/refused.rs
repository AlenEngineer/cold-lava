//! Requests that were not done, so that what is in front keeps what it holds
//! (docs/foundations.md, "Unsaved work is never taken away"): how they are said, by the menu
//! and by the whiteboard. Each keeps its notice here, apart from its other messages, so that
//! no message takes a notice's place: it comes first on the line, and what else is said
//! follows it. A line too narrow for the notice cuts its end.

use std::time::{Duration, Instant};

/// How long the notice shows, from when it first could.
pub const SHOWS: Duration = Duration::from_secs(3);

/// `notice`, standing for `n` refused requests.
pub fn counted(notice: String, n: usize) -> String {
    match n {
        0 | 1 => notice,
        n => format!("{notice} ({n} requests refused)"),
    }
}

/// The notice of refused requests: the last one's words, and how many it stands for.
#[derive(Default)]
pub struct Refused {
    last: Option<Last>,
}

struct Last {
    notice: String,
    n: usize,
    /// When it first showed: its time runs from then. `None` while it waits to be shown.
    since: Option<Instant>,
}

impl Last {
    fn unread(&self, now: Instant) -> bool {
        self.since.is_none_or(|at| now.duration_since(at) <= SHOWS)
    }
}

impl Refused {
    /// A request was refused at `now`, for the reason `notice` gives. One that comes while
    /// the last notice still shows, or waits to, takes its place and is counted with it. Only
    /// the last request is named.
    pub fn add(&mut self, notice: String, now: Instant) {
        let n = match &self.last {
            Some(last) if last.unread(now) => last.n + 1,
            _ => 1,
        };
        self.last = Some(Last {
            notice,
            n,
            since: None,
        });
    }

    /// The notice is on the screen at `now`: its time begins, if it hasn't.
    pub fn begin(&mut self, now: Instant) {
        if let Some(last) = &mut self.last
            && last.since.is_none()
        {
            last.since = Some(now);
        }
    }

    /// What to say at `now`: the last notice with its count, until `SHOWS` after it began.
    pub fn shown(&self, now: Instant) -> Option<String> {
        let last = self.last.as_ref().filter(|last| last.unread(now))?;
        Some(counted(last.notice.clone(), last.n))
    }

    /// The line a status line says at `now`: the notice first, then `other`, what else there
    /// is to say, so that nothing pushes the notice off the line.
    pub fn line(&self, other: Option<&str>, now: Instant) -> Option<String> {
        match (self.shown(now), other) {
            (Some(refused), Some(other)) => Some(format!("{refused} · {other}")),
            (Some(refused), None) => Some(refused),
            (None, other) => other.map(str::to_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_notice_shows_for_three_seconds_and_counts_the_requests_that_came_meanwhile() {
        let start = Instant::now();
        let mut refused = Refused::default();
        assert_eq!(refused.shown(start), None);

        refused.add("a wanted to open X: not while typing".into(), start);
        refused.begin(at(start, 10));
        assert_eq!(
            refused.shown(at(start, 10)).as_deref(),
            Some("a wanted to open X: not while typing")
        );

        refused.add(
            "b wanted to run y: not while typing".into(),
            at(start, 2000),
        );
        refused.begin(at(start, 2010));
        assert_eq!(
            refused.shown(at(start, 5000)).as_deref(),
            Some("b wanted to run y: not while typing (2 requests refused)"),
            "three seconds from when the last one showed, and both are counted"
        );
        assert_eq!(refused.shown(at(start, 5011)), None);

        refused.add(
            "a wanted to open X: not while typing".into(),
            at(start, 6000),
        );
        assert_eq!(
            refused.shown(at(start, 6001)).as_deref(),
            Some("a wanted to open X: not while typing"),
            "the count starts again once a notice had its time"
        );
    }

    #[test]
    fn a_notice_that_cant_show_yet_waits_and_its_time_begins_when_it_does() {
        let start = Instant::now();
        let mut refused = Refused::default();
        refused.add("first".into(), start);
        assert_eq!(
            refused.shown(at(start, 60_000)).as_deref(),
            Some("first"),
            "not shown yet, so not over"
        );
        refused.add("second".into(), at(start, 60_000));
        refused.begin(at(start, 61_000));
        refused.begin(at(start, 62_000));
        assert_eq!(
            refused.shown(at(start, 64_000)).as_deref(),
            Some("second (2 requests refused)")
        );
        assert_eq!(refused.shown(at(start, 64_001)), None);
    }

    #[test]
    fn the_notice_comes_first_on_the_line() {
        let start = Instant::now();
        let mut refused = Refused::default();
        assert_eq!(refused.line(None, start), None);
        assert_eq!(refused.line(Some("saved"), start).as_deref(), Some("saved"));
        refused.add("a wanted to open X".into(), start);
        assert_eq!(
            refused.line(Some("saved"), start).as_deref(),
            Some("a wanted to open X · saved")
        );
        assert_eq!(
            refused.line(None, start).as_deref(),
            Some("a wanted to open X")
        );
    }
}
