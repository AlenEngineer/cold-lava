//! What owned files leave out, as the menu says it (`core::Event::OwnedLeavesOut`). A board's
//! report waits until the status line in front has nothing else to say, so it never takes the
//! place of what the user just did (#303), and the boards' reports that wait together are said
//! in one line that names the boards, once for the time a message shows (the owner's decision
//! of 2026-11-20). What an extension's file leaves out, and an owner that couldn't read a file,
//! are said at once, as before #303, and the boards' line one of them takes the place of is
//! said again after it.

use crate::core::Event;
use crate::notice::Notice;
use crate::owned::couldnt_read;
use std::collections::VecDeque;
use std::path::Path;

/// A board's report: the board (workspace-relative) and what it leaves out.
type Report = (String, Notice);

/// The boards' reports, waiting for the status line or said on it.
#[derive(Default)]
pub struct Reports {
    /// Waiting for a status line with nothing else to say, oldest first.
    waiting: VecDeque<Report>,
    /// The line `pop` gave last, with the reports it says: they wait again when a report said
    /// at once takes its place (`heard`).
    given: Option<(Notice, Vec<Report>)>,
}

impl Reports {
    /// What to say now of what the core said: that an owner couldn't read a file
    /// (`Event::OwnerTrouble`), or what an extension's file leaves out
    /// (`Event::OwnedLeavesOut`). It takes the place of `front`, what the status line in front
    /// says, and when that is the line `pop` gave last, the line's reports wait again, to be
    /// said after it. A board's report waits instead, in the place of one about that board,
    /// which is dropped once the board leaves nothing out (`None`).
    pub fn heard(&mut self, event: &Event, front: Option<&Notice>) -> Option<Notice> {
        let now = match event {
            Event::OwnerTrouble {
                owner,
                path,
                message,
            } => couldnt_read(owner, path, message),
            Event::OwnedLeavesOut {
                owner,
                path,
                message,
            } if crate::owner::is_board(path) => {
                self.forget(path);
                if let Some(message) = message {
                    let report = Notice::from(couldnt_read(owner, path, message));
                    self.waiting.push_back((path.clone(), report));
                }
                return None;
            }
            Event::OwnedLeavesOut {
                owner,
                path,
                message: Some(message),
            } => couldnt_read(owner, path, message),
            _ => return None,
        };
        if let Some((_, cut)) = self.given.take_if(|given| front == Some(&given.0)) {
            for report in cut.into_iter().rev() {
                self.waiting.push_front(report);
            }
        }
        Some(now.into())
    }

    /// What to say next, on a status line with nothing else to say: the one board's report
    /// that waits, or one line that names every board that waits.
    pub fn pop(&mut self) -> Option<Notice> {
        let reports: Vec<Report> = self.waiting.drain(..).collect();
        let line = match reports.as_slice() {
            [] => return None,
            [(_, report)] => report.clone(),
            _ => named(&reports),
        };
        self.given = Some((line.clone(), reports));
        Some(line)
    }

    /// Drops what waits about the board at `path`, and the board from the line given last.
    fn forget(&mut self, path: &str) {
        self.waiting.retain(|(board, _)| board != path);
        if let Some((_, reports)) = &mut self.given {
            reports.retain(|(board, _)| board != path);
        }
    }
}

/// One line that names the boards of `reports`: how many there are and where each is shown
/// first, so that a line cut at the terminal's width keeps them, then each board by its name
/// (`board::name`), or by its path where another has the same name.
fn named(reports: &[Report]) -> Notice {
    let names: Vec<String> = (reports.iter())
        .map(|(path, _)| crate::board::name(Path::new(path)))
        .collect();
    let told: Vec<&str> = (reports.iter().zip(&names))
        .map(|((path, _), name)| {
            let alone = names.iter().filter(|n| *n == name).count() == 1;
            if alone { name.as_str() } else { path.as_str() }
        })
        .collect();
    let n = reports.len();
    let told = told.join(", ");
    format!("{n} boards have cards left out (cold-lava items shows each): {told}").into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the core says a file leaves out (`None`: nothing).
    fn left(owner: &str, path: &str, message: Option<&str>) -> Event {
        Event::OwnedLeavesOut {
            owner: owner.into(),
            path: path.into(),
            message: message.map(String::from),
        }
    }

    /// What the core says of the board at `path`, whose first card has an empty id.
    fn board(path: &str) -> Event {
        left("canvas", path, Some("card 1 (id \"\") is left out"))
    }

    #[test]
    fn a_boards_report_waits_and_boards_that_wait_together_are_named_in_one_line() {
        let mut reports = Reports::default();
        assert_eq!(reports.pop(), None);
        let csv = left("csv-table", "Eruptions.csv", Some("row 3 is left out"));
        assert_eq!(
            reports.heard(&csv, None),
            Some("csv-table couldn't read Eruptions.csv: row 3 is left out".into()),
            "an extension's, at once"
        );
        let mended = left("csv-table", "Eruptions.csv", None);
        assert_eq!(reports.heard(&mended, None), None);
        let broken = Event::OwnerTrouble {
            owner: "canvas".into(),
            path: "Broken.canvas".into(),
            message: "it isn't a JSON Canvas".into(),
        };
        assert_eq!(
            reports.heard(&broken, None),
            Some("canvas couldn't read Broken.canvas: it isn't a JSON Canvas".into()),
            "a board that doesn't read, at once"
        );
        assert_eq!(reports.pop(), None, "neither waits");

        assert_eq!(reports.heard(&board("Lava/Ashfall.canvas"), None), None);
        assert_eq!(
            reports.pop(),
            Some("canvas couldn't read Lava/Ashfall.canvas: card 1 (id \"\") is left out".into()),
            "one board's own report"
        );
        assert_eq!(reports.pop(), None);

        for path in [
            "Ashfall.canvas",
            "Lava/Lahar.canvas",
            "Mended.canvas",
            "Ashfall.canvas",
        ] {
            reports.heard(&board(path), None);
        }
        reports.heard(&left("canvas", "Mended.canvas", None), None);
        assert_eq!(
            reports.pop(),
            Some(
                "2 boards have cards left out (cold-lava items shows each): Lahar, Ashfall".into()
            ),
            "a newer report takes a waiting one's place, and a mended board's is dropped"
        );
        assert_eq!(reports.pop(), None);

        for path in ["Ashfall.canvas", "Lava/Ashfall.canvas", "Lava/Lahar.canvas"] {
            reports.heard(&board(path), None);
        }
        assert_eq!(
            reports.pop(),
            Some(
                "3 boards have cards left out (cold-lava items shows each): Ashfall.canvas, \
                 Lava/Ashfall.canvas, Lahar"
                    .into()
            ),
            "two boards of one name go by their paths"
        );
    }

    /// The owner's decision of 2026-11-20: the boards' line is shown once for 3 s, so a
    /// report said at once that takes its place puts it back in the queue; what the user did
    /// doesn't.
    #[test]
    fn the_boards_line_waits_again_when_a_report_said_at_once_takes_its_place() {
        let mut reports = Reports::default();
        for path in ["Ashfall.canvas", "Lahar.canvas", "Mended.canvas"] {
            reports.heard(&board(path), None);
        }
        let line = reports.pop().expect("the boards' line");
        reports.heard(&left("canvas", "Mended.canvas", None), Some(&line));
        let trouble = Event::OwnerTrouble {
            owner: "csv-table".into(),
            path: "Eruptions.csv".into(),
            message: "it stopped".into(),
        };
        let said: Option<Notice> = Some("csv-table couldn't read Eruptions.csv: it stopped".into());
        assert_eq!(reports.heard(&trouble, Some(&line)), said, "at once");
        assert_eq!(reports.heard(&trouble, said.as_ref()), said);
        assert_eq!(
            reports.pop(),
            Some(
                "2 boards have cards left out (cold-lava items shows each): Ashfall, Lahar".into()
            ),
            "said again, without the board mended meanwhile"
        );
        assert_eq!(reports.pop(), None, "once");

        let done: Notice = "bookmarked Krakatoa".into();
        assert_eq!(reports.heard(&trouble, Some(&done)), said);
        assert_eq!(
            reports.pop(),
            None,
            "what the user did took the line's place first"
        );
    }
}
