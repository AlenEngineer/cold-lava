//! Link hints: every link on screen gets a label of one or two letters; typing a label picks
//! its link.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::app::{ACCENT, DIM};

/// Labels come from the home row while it has enough of them, in this order.
const HOME_ROW: &str = "asdfjkl;";
const OTHERS: &str = "ghqwertyuiopzxcvbnm";

/// `n` labels, none the start of another, shortest first: single home-row letters while
/// there are enough, else the first letters become the starts of pairs, one at a time, until
/// there are enough. Past 64 labels, any letter goes.
pub fn labels(n: usize) -> Vec<String> {
    let home = HOME_ROW.chars().count();
    let letters: Vec<char> = if n <= home * home {
        HOME_ROW.chars().collect()
    } else {
        HOME_ROW.chars().chain(OTHERS.chars()).collect()
    };
    let mut labels = vec![String::new()];
    let mut used = 0;
    while labels.len() - used < n || used == 0 {
        let start = labels[used].clone();
        used += 1;
        labels.extend(letters.iter().map(|c| format!("{start}{c}")));
    }
    labels.drain(used..used + n).collect()
}

pub struct Hints<T> {
    labelled: Vec<(String, T)>,
    typed: String,
}

pub enum Typed<T> {
    /// Some labels start with what was typed; keep going.
    Pending,
    Chosen(T),
    /// No label starts with it.
    NoMatch,
}

impl<T: Copy> Hints<T> {
    /// Labels `targets`, the shortest labels going to the first.
    pub fn new(targets: impl IntoIterator<Item = T>) -> Hints<T> {
        let targets: Vec<T> = targets.into_iter().collect();
        Hints {
            labelled: labels(targets.len()).into_iter().zip(targets).collect(),
            typed: String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.labelled.is_empty()
    }

    pub fn type_char(&mut self, c: char) -> Typed<T> {
        self.typed.extend(c.to_lowercase());
        let mut matching = self.matching();
        match (matching.next(), matching.next()) {
            (None, _) => Typed::NoMatch,
            (Some((label, target)), None) if label == self.typed => Typed::Chosen(*target),
            _ => Typed::Pending,
        }
    }

    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// The labels that still match what was typed, with their targets.
    pub fn matching(&self) -> impl Iterator<Item = (&str, &T)> {
        self.labelled
            .iter()
            .filter(|(label, _)| label.starts_with(&self.typed))
            .map(|(label, target)| (label.as_str(), target))
    }

    /// Draws `label` over what is at `x`, `y` (clipped to `area`), its typed part dimmed.
    pub fn draw(&self, buf: &mut Buffer, area: Rect, x: u16, y: u16, label: &str) {
        if !area.contains((x, y).into()) {
            return;
        }
        let width = (area.right() - x) as usize;
        let (done, rest) = label.split_at(self.typed.len());
        let base = Style::new().bg(ACCENT).add_modifier(Modifier::BOLD);
        buf.set_stringn(x, y, done, width, base.fg(DIM));
        let x = x + done.len() as u16;
        if x < area.right() {
            buf.set_stringn(x, y, rest, width - done.len(), base.fg(Color::Black));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn few_links_get_single_home_row_letters() {
        assert_eq!(labels(3), ["a", "s", "d"]);
        assert_eq!(labels(8).last().unwrap(), ";");
        assert!(labels(0).is_empty());
    }

    #[test]
    fn more_links_turn_first_letters_into_pairs() {
        let l = labels(11);
        assert_eq!(
            l,
            ["s", "d", "f", "j", "k", "l", ";", "aa", "as", "ad", "af"]
        );
        assert!(labels(64).iter().all(|x| x.len() == 2));
        for n in [9, 30, 64, 65, 300] {
            let l = labels(n);
            assert_eq!(l.len(), n);
            assert!(
                l.iter().all(|x| x.len() <= 2),
                "{n} links need at most two letters"
            );
            for a in &l {
                let starts = l.iter().filter(|b| b.starts_with(a.as_str())).count();
                assert_eq!(starts, 1, "{a} starts only itself, of {n}");
            }
        }
    }

    #[test]
    fn typing_a_label_chooses_its_target() {
        let mut h = Hints::new(0..3);
        assert!(matches!(h.type_char('S'), Typed::Chosen(1)));
        let mut h = Hints::new(0..10);
        assert!(matches!(h.type_char('a'), Typed::Pending));
        assert_eq!(h.matching().count(), 3);
        assert!(matches!(h.type_char('s'), Typed::Chosen(8)));
    }

    #[test]
    fn a_letter_no_label_starts_with_is_no_match() {
        let mut h = Hints::new(0..3);
        assert!(matches!(h.type_char('j'), Typed::NoMatch));
        assert_eq!(h.typed(), "j");
        let mut h = Hints::new(0..10);
        assert!(matches!(h.type_char('a'), Typed::Pending));
        assert!(matches!(h.type_char('x'), Typed::NoMatch));
    }
}
