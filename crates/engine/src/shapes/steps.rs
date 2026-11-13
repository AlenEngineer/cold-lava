//! The rules of an `on` entry's steps. A shape's definition (`do`, `edit`) and an extension's
//! answer to `shape/gesture` say steps in one language, so the rules are here once, and `Steps`
//! is what passed them all: the only steps `Binding::run` takes.
//!
//! There are two rules. `edits` is what a step gets wrong on its own. `unknown_rebinds` needs
//! the loaded shapes, so the loader, which reads every layer before it knows them, asks the
//! two at two times. `Steps::checked` asks both at once.

use super::{EditStep, SetField, Shapes, Step};
use crate::problem::Problem;

/// Steps that passed both rules. Only `checked` makes one, so a step that runs was checked,
/// whether a definition or an extension wrote it.
#[derive(Clone, Debug, PartialEq)]
pub struct Steps(Vec<Step>);

impl Steps {
    /// What `steps` get wrong on their own, which needs no shapes: an answer's steps are
    /// checked for it as they arrive. Empty when they pass.
    pub fn alone(steps: &[Step]) -> Vec<Problem> {
        edits(steps)
    }

    /// A problem for each rebind of `steps` to a shape that isn't among the loaded `shapes`.
    pub fn unknown(steps: &[Step], shapes: &Shapes) -> Vec<Problem> {
        unknown_rebinds(steps, &|id: &str| shapes.get(id).is_some())
    }

    /// `steps` among the loaded `shapes`, or what they get wrong, one problem each: what
    /// they get wrong on their own (`alone`, the rules an answer's steps meet as it arrives),
    /// then their rebinds to shapes that aren't loaded.
    pub fn checked(steps: Vec<Step>, shapes: &Shapes) -> Result<Steps, Vec<Problem>> {
        let mut problems = Self::alone(&steps);
        problems.extend(Self::unknown(&steps, shapes));
        match problems.is_empty() {
            true => Ok(Steps(steps)),
            false => Err(problems),
        }
    }
}

impl std::ops::Deref for Steps {
    type Target = [Step];

    fn deref(&self) -> &[Step] {
        &self.0
    }
}

/// What steps get wrong on their own, one problem each, in the steps' order.
pub(super) fn edits(steps: &[Step]) -> Vec<Problem> {
    (steps.iter())
        .flat_map(|step| match step {
            Step::Edit(e) => check_edit(e),
            Step::Create(_) | Step::Rebind(_) => Vec::new(),
        })
        .collect()
}

/// A problem for each `rebind` to a shape that isn't loaded, as `knows` says, in the steps'
/// order.
pub(super) fn unknown_rebinds(steps: &[Step], knows: &dyn Fn(&str) -> bool) -> Vec<Problem> {
    (steps.iter())
        .filter_map(|step| match step {
            Step::Rebind(r) if !knows(r.shape.as_str()) => Some(Problem::line(format!(
                "its rebind names shape {}, which isn't loaded",
                r.shape
            ))),
            _ => None,
        })
        .collect()
}

fn check_edit(e: &EditStep) -> Vec<Problem> {
    let mut p = Vec::new();
    let kinds = [
        e.set_field.is_some(),
        e.toggle.is_some(),
        e.recipe.is_some(),
    ];
    if kinds.iter().filter(|k| **k).count() != 1 {
        p.push(Problem::line(
            "an edit is exactly one of set-field, toggle and recipe",
        ));
    }
    match (&e.recipe, &e.item) {
        (None, None) => p.push(Problem::line(
            "say which item set-field or toggle changes, like item = \"dropped\"",
        )),
        (Some(_), Some(_)) => p.push(Problem::line(
            "a recipe changes the whole workspace, so it names no item",
        )),
        _ => {}
    }
    if e.recipe.is_none() && !e.options.is_empty() {
        p.push(Problem::line("options are a recipe's"));
    }
    if let Some(Err(problem)) = e.set_field.as_ref().map(SetField::way) {
        p.push(Problem::line(problem));
    }
    p
}

/// What a `set-field` does with its field: sets it, or adds to or takes from its list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Way<'a> {
    Value(&'a str),
    Add(&'a str),
    Remove(&'a str),
}

impl SetField {
    /// The one of `value`, `add` and `remove` it has, or that it doesn't have exactly one.
    pub fn way(&self) -> Result<Way<'_>, &'static str> {
        match (&self.value, &self.add, &self.remove) {
            (Some(v), None, None) => Ok(Way::Value(v.as_str())),
            (None, Some(v), None) => Ok(Way::Add(v.as_str())),
            (None, None, Some(v)) => Ok(Way::Remove(v.as_str())),
            _ => Err("set-field takes exactly one of value, add and remove"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{Layer, load};
    use serde_json::json;

    fn steps(json: serde_json::Value) -> Vec<Step> {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn steps_that_can_run_pass_and_the_rest_say_what_is_wrong_in_order() {
        let shapes = load([Layer::built_in()]);
        let good = steps(json!([
            {"edit": {"item": "dropped", "set-field": {"field": "tags", "add": "seen"}}},
            {"edit": {"item": "clicked", "toggle": true}},
            {"edit": {"recipe": "rename-tag", "options": {"from": "a", "to": "b"}}},
            {"create": {"text": "# New"}},
            {"rebind": {"shape": "card", "inputs": {"note": "created"}}},
        ]));
        assert_eq!(
            Steps::checked(good.clone(), &shapes).as_deref(),
            Ok(&good[..])
        );
        assert!(Steps::checked(Vec::new(), &shapes).is_ok_and(|none| none.is_empty()));

        let bad = steps(json!([
            {"edit": {"item": "clicked", "set-field": {"field": "visited"}}},
            {"create": {"text": "# New"}},
            {"edit": {"options": {"from": "a"}}},
            {"edit": {"item": "clicked", "recipe": "rename-tag", "toggle": true}},
            {"edit": {"item": "clicked", "set-field": {"field": "tags", "add": "a", "remove": "b"}}},
            {"rebind": {"shape": "nope"}},
            {"rebind": {"shape": "card"}},
            {"rebind": {"shape": "gone"}},
        ]));
        assert_eq!(
            Steps::checked(bad, &shapes),
            Err(vec![
                Problem::line("set-field takes exactly one of value, add and remove"),
                Problem::line("an edit is exactly one of set-field, toggle and recipe"),
                Problem::line(
                    "say which item set-field or toggle changes, like item = \"dropped\""
                ),
                Problem::line("options are a recipe's"),
                Problem::line("an edit is exactly one of set-field, toggle and recipe"),
                Problem::line("a recipe changes the whole workspace, so it names no item"),
                Problem::line("set-field takes exactly one of value, add and remove"),
                Problem::line("its rebind names shape nope, which isn't loaded"),
                Problem::line("its rebind names shape gone, which isn't loaded"),
            ])
        );
    }
}
