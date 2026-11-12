//! Requests that were not done, so that what is in front keeps what it holds
//! (docs/foundations.md, "Unsaved work is never taken away"): how they are said. A refused
//! request is always shown, so the menu keeps its notice apart from the status line's other
//! messages, which don't take its place, and counts the requests it stands for.

use std::time::{Duration, Instant};

/// How long the notice shows after the last request it counts.
pub const SHOWS: Duration = Duration::from_secs(3);

/// `notice`, standing for `n` refused requests: the one way to say so, the whiteboard's too.
pub fn counted(notice: String, n: usize) -> String {
    match n {
        0 | 1 => notice,
        n => format!("{notice} ({n} requests refused)"),
    }
}

/// The menu's notice of refused requests.
#[derive(Default)]
pub struct Refused {
    /// The last request's notice, how many it stands for, and when the last one came.
    last: Option<(String, usize, Instant)>,
}

impl Refused {
    /// A request was refused at `now`, for the reason `notice` gives. One that comes while
    /// the last notice still shows takes its place and is counted with it.
    pub fn add(&mut self, notice: String, now: Instant) {
        let n = match &self.last {
            Some((_, n, at)) if now.duration_since(*at) <= SHOWS => n + 1,
            _ => 1,
        };
        self.last = Some((notice, n, now));
    }

    /// What the status line says at `now`: the last notice with its count, until `SHOWS`
    /// after the last request.
    pub fn shown(&mut self, now: Instant) -> Option<String> {
        let over = |(_, _, at): &(String, usize, Instant)| now.duration_since(*at) > SHOWS;
        if self.last.as_ref().is_some_and(over) {
            self.last = None;
        }
        let (notice, n, _) = self.last.as_ref()?;
        Some(counted(notice.clone(), *n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_shows_for_three_seconds_and_counts_the_requests_that_came_meanwhile() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut refused = Refused::default();
        assert_eq!(refused.shown(at(0)), None);

        refused.add("a wanted to open X: not while typing".into(), at(0));
        assert_eq!(
            refused.shown(at(10)).as_deref(),
            Some("a wanted to open X: not while typing")
        );

        refused.add("b wanted to run y: not while typing".into(), at(2000));
        assert_eq!(
            refused.shown(at(4500)).as_deref(),
            Some("b wanted to run y: not while typing (2 requests refused)"),
            "three seconds from the last request, and both are counted"
        );
        assert_eq!(refused.shown(at(5001)), None);

        refused.add("a wanted to open X: not while typing".into(), at(6000));
        assert_eq!(
            refused.shown(at(6001)).as_deref(),
            Some("a wanted to open X: not while typing"),
            "the count starts again once a notice was shown out"
        );
    }

    #[test]
    fn a_request_after_the_notice_ran_out_unseen_starts_a_new_count() {
        let start = Instant::now();
        let mut refused = Refused::default();
        refused.add("first".into(), start);
        refused.add("second".into(), start + SHOWS + Duration::from_millis(1));
        assert_eq!(
            refused
                .shown(start + SHOWS + Duration::from_millis(2))
                .as_deref(),
            Some("second")
        );
    }
}
