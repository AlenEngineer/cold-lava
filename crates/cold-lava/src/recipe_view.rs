//! The palette's `recipe.run`: pick a recipe, fill its options, see its plan (every file with
//! its diff) and apply it or not. The menu plans and applies; this is what is on screen and
//! what a key asks for.

use crate::app::{ACCENT, HIT, MUTED, NOTE, TAG};
use crate::gateway::Ticket;
use crate::notice::Notice;
use crate::recipe::{self, Options, Plan, Recipe};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Where the dialog is.
pub enum Step {
    Pick {
        sel: usize,
    },
    Options {
        recipe: &'static Recipe,
        values: Vec<String>,
        at: usize,
        /// Why the last plan was refused.
        error: Option<Notice>,
    },
    Plan {
        plan: Box<Plan>,
        scroll: usize,
        /// Confirmed, and waiting for the owners of files other owners read (`Core::relink`);
        /// nothing is written yet.
        waiting: Option<Ticket>,
    },
}

/// What the menu should do after a key.
pub enum Action {
    Stay,
    /// Closed; with a ticket, stop waiting for its owners.
    Close(Option<Ticket>),
    /// Plan this recipe with these options.
    Plan(&'static Recipe, Options),
    /// Apply the plan shown.
    Apply,
}

pub struct RecipeView {
    pub step: Step,
}

impl RecipeView {
    pub fn new() -> RecipeView {
        RecipeView {
            step: Step::Pick { sel: 0 },
        }
    }

    /// What the dialog holds that closing it would lose, in words for the status line:
    /// options being filled, or a plan shown or waiting for owners. The list of recipes holds
    /// nothing.
    pub fn unsaved(&self) -> Option<&'static str> {
        match &self.step {
            Step::Pick { .. } => None,
            Step::Options { .. } => Some("a recipe's options are being typed"),
            Step::Plan { waiting: None, .. } => Some("a recipe waits for your answer"),
            Step::Plan {
                waiting: Some(_), ..
            } => Some("a recipe waits for its files' owners"),
        }
    }

    /// The options step again, saying why the plan was refused.
    pub fn refused(&mut self, why: String) {
        if let Step::Options { error, .. } = &mut self.step {
            *error = Some(why.into());
        }
    }

    pub fn key(&mut self, k: KeyEvent) -> Action {
        match &mut self.step {
            Step::Pick { sel } => match k.code {
                KeyCode::Esc => return Action::Close(None),
                KeyCode::Down => *sel = (*sel + 1).min(recipe::BUILT_IN.len() - 1),
                KeyCode::Up => *sel = sel.saturating_sub(1),
                KeyCode::Enter => {
                    let recipe = &recipe::BUILT_IN[*sel];
                    self.step = Step::Options {
                        recipe,
                        values: vec![String::new(); recipe.options.len()],
                        at: 0,
                        error: None,
                    };
                }
                _ => {}
            },
            Step::Options {
                recipe,
                values,
                at,
                error,
            } => match k.code {
                KeyCode::Esc => return Action::Close(None),
                KeyCode::Tab | KeyCode::Down => *at = (*at + 1) % values.len(),
                KeyCode::BackTab | KeyCode::Up => *at = (*at + values.len() - 1) % values.len(),
                KeyCode::Backspace => {
                    values[*at].pop();
                }
                KeyCode::Char(c) => {
                    values[*at].push(c);
                    *error = None;
                }
                KeyCode::Enter => match values.iter().position(|v| v.trim().is_empty()) {
                    Some(empty) => *at = empty,
                    None => {
                        let options = (recipe.options.iter())
                            .zip(values.iter())
                            .map(|(o, v)| (o.name.to_string(), v.trim().to_string()))
                            .collect();
                        return Action::Plan(recipe, options);
                    }
                },
                _ => {}
            },
            Step::Plan {
                plan,
                scroll,
                waiting,
            } => match k.code {
                KeyCode::Esc | KeyCode::Char('n') => return Action::Close(*waiting),
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown => *scroll += 10,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::Enter | KeyCode::Char('y')
                    if waiting.is_none() && !plan.edits.is_empty() =>
                {
                    return Action::Apply;
                }
                _ => {}
            },
        }
        Action::Stay
    }

    pub fn title(&self) -> String {
        match &self.step {
            Step::Pick { .. } => " Recipe ".into(),
            Step::Options { recipe, .. } => format!(" Recipe · {} ", recipe.id),
            Step::Plan { plan, .. } => format!(" Recipe · {} ", plan.title()),
        }
    }

    /// What the keys do now; `undo` is the undo key's label.
    pub fn hint(&self, undo: &str) -> String {
        match &self.step {
            Step::Pick { .. } => " ↑↓ choose · ⏎ its options · Esc cancel ".into(),
            Step::Options { .. } => " Tab next · ⏎ preview · Esc cancel ".into(),
            Step::Plan {
                waiting: Some(_),
                plan,
                ..
            } => {
                let owners: Vec<&str> = (plan.edits.iter())
                    .filter_map(|e| e.owned.as_ref().filter(|o| o.after.is_none()))
                    .map(|o| o.owner.as_str())
                    .collect();
                format!(
                    " waiting for {} to update its files · Esc stop ",
                    owners.join(", ")
                )
            }
            Step::Plan { plan, .. } if plan.edits.is_empty() => " Esc close ".into(),
            Step::Plan { .. } => {
                format!(" ⏎ apply · Esc cancel · ↑↓ scroll · {undo} undoes it after ")
            }
        }
    }

    pub fn lines(&self, width: usize, height: usize) -> Vec<Line<'static>> {
        let text = Style::new().fg(NOTE);
        let muted = Style::new().fg(MUTED);
        match &self.step {
            Step::Pick { sel } => (recipe::BUILT_IN.iter().enumerate())
                .flat_map(|(i, r)| {
                    let mark = if i == *sel { "▸ " } else { "  " };
                    let name = match i == *sel {
                        true => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                        false => text,
                    };
                    [
                        Line::from(vec![Span::styled(format!(" {mark}{}", r.id), name)]),
                        Line::styled(format!("     {}", r.about), muted),
                    ]
                })
                .collect(),
            Step::Options {
                recipe,
                values,
                at,
                error,
            } => {
                let mut lines = vec![
                    Line::styled(format!(" {}", recipe.about), muted),
                    Line::raw(""),
                ];
                let wide = recipe
                    .options
                    .iter()
                    .map(|o| o.name.len())
                    .max()
                    .unwrap_or(0);
                for (i, (o, v)) in recipe.options.iter().zip(values).enumerate() {
                    let here = i == *at;
                    let label = match here {
                        true => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                        false => text,
                    };
                    let cursor = if here { "▏" } else { "" };
                    lines.push(Line::from(vec![
                        Span::styled(format!(" {:wide$}  ", o.name), label),
                        Span::styled(format!("{v}{cursor}"), text),
                    ]));
                    lines.push(Line::styled(format!(" {:wide$}  {}", "", o.about), muted));
                }
                if let Some(why) = error {
                    lines.push(Line::raw(""));
                    lines.push(Line::styled(format!(" ! {why}"), Style::new().fg(HIT)));
                }
                lines
            }
            Step::Plan { plan, scroll, .. } => {
                let mut head = vec![Line::styled(
                    match plan.edits.is_empty() {
                        true => " Nothing to change.".to_string(),
                        false => format!(" Changes {}:", plan.what()),
                    },
                    text,
                )];
                for w in &plan.warnings {
                    head.push(Line::styled(format!(" ! {w}"), Style::new().fg(HIT)));
                }
                head.push(Line::raw(""));
                let mut detail = Vec::new();
                for e in &plan.edits {
                    let through = match &e.owned {
                        Some(o) => format!(" (through {})", o.owner),
                        None => String::new(),
                    };
                    detail.push(Line::styled(
                        format!(" {}{through}", e.file),
                        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                    ));
                    for d in recipe::file_diff(e, 1) {
                        let Some(d) = d else {
                            detail.push(Line::styled("   ⋯", muted));
                            continue;
                        };
                        let style = match d.op {
                            '-' => Style::new().fg(HIT),
                            '+' => Style::new().fg(TAG),
                            _ => muted,
                        };
                        let line: String = format!(" {:>4} {} {}", d.line, d.op, d.text)
                            .chars()
                            .take(width)
                            .collect();
                        detail.push(Line::styled(line, style));
                    }
                }
                let room = height.saturating_sub(head.len());
                let start = (*scroll).min(detail.len().saturating_sub(room));
                head.extend(detail.into_iter().skip(start).take(room));
                head
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_and_a_plan_are_held_and_the_list_of_recipes_is_not() {
        let recipe = &recipe::BUILT_IN[0];
        let mut view = RecipeView::new();
        assert_eq!(view.unsaved(), None);

        view.step = Step::Options {
            recipe,
            values: Vec::new(),
            at: 0,
            error: None,
        };
        assert_eq!(view.unsaved(), Some("a recipe's options are being typed"));

        let plan = |waiting| Step::Plan {
            plan: Box::new(Plan {
                recipe,
                options: Options::new(),
                edits: Vec::new(),
                warnings: Vec::new(),
            }),
            scroll: 0,
            waiting,
        };
        view.step = plan(None);
        assert_eq!(view.unsaved(), Some("a recipe waits for your answer"));
        view.step = plan(Some(Ticket(1)));
        assert_eq!(view.unsaved(), Some("a recipe waits for its files' owners"));
    }
}
