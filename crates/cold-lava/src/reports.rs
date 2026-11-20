//! What owned files leave out, as the menu says it (`core::Event::LeftOut`). A board's report
//! waits until the status line in front has nothing else to say, so it never takes the place
//! of what the user just did (#303), and the boards' reports that wait together are said in
//! one line that names the boards (the owner's decision of 2026-11-20). What an extension's
//! file leaves out is said at once, as before #303.

use crate::notice::Notice;
use std::collections::VecDeque;
use std::path::Path;

/// The boards' reports waiting for the status line, oldest first, each with its board.
#[derive(Default)]
pub struct Reports(VecDeque<(String, Notice)>);

impl Reports {
    /// The core said what the file at `path`, owned by `owner`, leaves out now (`message`;
    /// `None`: nothing). A board's report waits (`pop`) and takes the place of one about that
    /// board still waiting, which is dropped when the board leaves nothing out any more. Any
    /// other file's report is returned, to say now.
    pub fn heard(&mut self, owner: &str, path: &str, message: Option<&str>) -> Option<Notice> {
        let report = message.map(|m| Notice::from(crate::owned::couldnt_read(owner, path, m)));
        if crate::owner::built_in(path).is_none() {
            return report;
        }
        self.0.retain(|(board, _)| board != path);
        self.0.extend(report.map(|r| (path.to_string(), r)));
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

    #[test]
    fn a_boards_report_waits_and_boards_that_wait_together_are_named_in_one_line() {
        let mut reports = Reports::default();
        assert_eq!(reports.pop(), None);
        let card = Some("card 1 (id \"\") is left out");
        assert_eq!(
            reports.heard("csv-table", "Eruptions.csv", Some("row 3 is left out")),
            Some("csv-table couldn't read Eruptions.csv: row 3 is left out".into()),
            "an extension's, at once"
        );
        assert_eq!(reports.heard("csv-table", "Eruptions.csv", None), None);
        assert_eq!(reports.pop(), None, "it doesn't wait");

        assert_eq!(reports.heard("canvas", "Lava/Ashfall.canvas", card), None);
        assert_eq!(
            reports.pop(),
            Some("canvas couldn't read Lava/Ashfall.canvas: card 1 (id \"\") is left out".into()),
            "one board's own report"
        );
        assert_eq!(reports.pop(), None);

        reports.heard("canvas", "Ashfall.canvas", card);
        reports.heard("canvas", "Lava/Lahar.canvas", card);
        reports.heard("canvas", "Mended.canvas", card);
        reports.heard("canvas", "Ashfall.canvas", card);
        reports.heard("canvas", "Mended.canvas", None);
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
