//! What owned files leave out, as the menu says it (`core::Event::LeftOut`). A board's report
//! waits until the status line in front has nothing else to say, so it never takes the place
//! of what the user just did (#303), and the boards' reports that wait together are said in
//! one line that names the boards (the owner's decision of 2026-11-20). What an extension's
//! file leaves out, and an owner that couldn't read a file, are said at once, as before #303.

use crate::core::Event;
use crate::notice::Notice;
use crate::owned::couldnt_read;
use std::collections::VecDeque;
use std::path::Path;

/// The boards' reports waiting for the status line, oldest first, each with its board.
#[derive(Default)]
pub struct Reports(VecDeque<(String, Notice)>);

impl Reports {
    /// What to say now of what the core said: that an owner couldn't read a file
    /// (`Event::OwnerTrouble`), or what an extension's file leaves out (`Event::LeftOut`). A
    /// board's report waits instead (`pop`), in the place of one about that board still
    /// waiting, which is dropped once the board leaves nothing out (`None`).
    pub fn heard(&mut self, event: &Event) -> Option<Notice> {
        let (owner, path, message) = match event {
            Event::OwnerTrouble {
                owner,
                path,
                message,
            } => return Some(couldnt_read(owner, path, message).into()),
            Event::LeftOut {
                owner,
                path,
                message,
            } => (owner, path, message),
            _ => return None,
        };
        let report = message
            .as_ref()
            .map(|m| couldnt_read(owner, path, m).into());
        if crate::owner::built_in(path).is_none() {
            return report;
        }
        self.0.retain(|(board, _)| board != path);
        self.0.extend(report.map(|r| (path.clone(), r)));
        None
    }

    /// What to say next, on a status line with nothing else to say: the one board's report
    /// that waits, or one line that names every board that waits.
    pub fn pop(&mut self) -> Option<Notice> {
        if self.0.len() < 2 {
            return self.0.pop_front().map(|(_, report)| report);
        }
        let names: Vec<String> = self.0.drain(..).map(|(board, _)| name(&board)).collect();
        let n = names.len();
        let names = names.join(", ");
        Some(format!("{n} boards have cards left out: {names} (cold-lava items shows each)").into())
    }
}

/// A board's name, as the whiteboard's title says it: its file's name without `.canvas`.
fn name(path: &str) -> String {
    let stem = Path::new(path).file_stem().unwrap_or_default();
    stem.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the core says a file leaves out (`None`: nothing).
    fn left(owner: &str, path: &str, message: Option<&str>) -> Event {
        Event::LeftOut {
            owner: owner.into(),
            path: path.into(),
            message: message.map(String::from),
        }
    }

    #[test]
    fn a_boards_report_waits_and_boards_that_wait_together_are_named_in_one_line() {
        let mut reports = Reports::default();
        assert_eq!(reports.pop(), None);
        let csv = left("csv-table", "Eruptions.csv", Some("row 3 is left out"));
        assert_eq!(
            reports.heard(&csv),
            Some("csv-table couldn't read Eruptions.csv: row 3 is left out".into()),
            "an extension's, at once"
        );
        assert_eq!(
            reports.heard(&left("csv-table", "Eruptions.csv", None)),
            None
        );
        let broken = Event::OwnerTrouble {
            owner: "canvas".into(),
            path: "Broken.canvas".into(),
            message: "it isn't a JSON Canvas".into(),
        };
        assert_eq!(
            reports.heard(&broken),
            Some("canvas couldn't read Broken.canvas: it isn't a JSON Canvas".into()),
            "a board that doesn't read, at once"
        );
        assert_eq!(reports.pop(), None, "neither waits");

        let card = Some("card 1 (id \"\") is left out");
        let board = |path: &str| left("canvas", path, card);
        assert_eq!(reports.heard(&board("Lava/Ashfall.canvas")), None);
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
            reports.heard(&board(path));
        }
        reports.heard(&left("canvas", "Mended.canvas", None));
        assert_eq!(
            reports.pop(),
            Some(
                "2 boards have cards left out: Lahar, Ashfall (cold-lava items shows each)".into()
            ),
            "a newer report takes a waiting one's place, and a mended board's is dropped"
        );
        assert_eq!(reports.pop(), None);
    }
}
