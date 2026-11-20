//! What owned files leave out, as the menu reports it (`core::Event::OwnerTrouble`): each
//! report waits until the status line in front has nothing else to say, so it never takes
//! the place of what the user just did, and several show one after another, each for the
//! time a message shows (#303).

use crate::notice::Notice;
use std::collections::VecDeque;

/// The reports waiting for the status line, oldest first, each with the file it is about.
#[derive(Default)]
pub struct Reports(VecDeque<(String, Notice)>);

impl Reports {
    /// A report about the file at `path`. One about that file still waiting says what is no
    /// longer so: this one takes its place, last in line.
    pub fn add(&mut self, path: &str, report: Notice) {
        self.0.retain(|(waiting, _)| waiting != path);
        self.0.push_back((path.to_string(), report));
    }

    /// The report to show next, for a status line that has nothing else to say.
    pub fn pop(&mut self) -> Option<Notice> {
        self.0.pop_front().map(|(_, report)| report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_wait_in_turn_and_a_newer_one_about_a_file_takes_its_place() {
        let mut reports = Reports::default();
        assert_eq!(reports.pop(), None);
        reports.add("A.canvas", "A: card 1".into());
        reports.add("B.canvas", "B: card 2".into());
        reports.add("A.canvas", "A: card 3".into());
        assert_eq!(reports.pop(), Some("B: card 2".into()));
        assert_eq!(reports.pop(), Some("A: card 3".into()));
        assert_eq!(reports.pop(), None);
    }
}
