//! The check of an `on` entry's steps. A shape's definition (`do`, `edit`) and an extension's
//! answer to `shape/gesture` say steps in one language, so one check reads both, and `Steps`
//! is what it lets through: the only steps `Binding::run` takes.

use super::{EditStep, SetField, Step};

/// Steps that passed `check_steps`. Nothing else makes one, so a step that runs was checked,
/// whether a definition or an extension wrote it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Steps(Vec<Step>);

impl Steps {
    /// `steps`, or what they get wrong, one line each.
    pub fn checked(steps: Vec<Step>) -> Result<Steps, Vec<String>> {
        let problems = check_steps(&steps);
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

/// What steps get wrong on their own, one line each, in the steps' order.
pub fn check_steps(steps: &[Step]) -> Vec<String> {
    (steps.iter())
        .flat_map(|step| match step {
            Step::Edit(e) => check_edit(e),
            Step::Create(_) | Step::Rebind(_) => Vec::new(),
        })
        .collect()
}

fn check_edit(e: &EditStep) -> Vec<String> {
    let mut p = Vec::new();
    let kinds = [
        e.set_field.is_some(),
        e.toggle.is_some(),
        e.recipe.is_some(),
    ];
    if kinds.iter().filter(|k| **k).count() != 1 {
        p.push("an edit is exactly one of set-field, toggle and recipe".to_string());
    }
    match (&e.recipe, &e.item) {
        (None, None) => p.push(
            "say which item set-field or toggle changes, like item = \"dropped\"".to_string(),
        ),
        (Some(_), Some(_)) => {
            p.push("a recipe changes the whole workspace, so it names no item".to_string())
        }
        _ => {}
    }
    if e.recipe.is_none() && !e.options.is_empty() {
        p.push("options are a recipe's".to_string());
    }
    if let Some(Err(problem)) = e.set_field.as_ref().map(SetField::way) {
        p.push(problem.to_string());
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
            (Some(v), None, None) => Ok(Way::Value(v)),
            (None, Some(v), None) => Ok(Way::Add(v)),
            (None, None, Some(v)) => Ok(Way::Remove(v)),
            _ => Err("set-field takes exactly one of value, add and remove"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn steps(json: serde_json::Value) -> Vec<Step> {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn steps_that_can_run_pass_and_the_rest_say_what_is_wrong_in_order() {
        let good = steps(json!([
            {"edit": {"item": "dropped", "set-field": {"field": "tags", "add": "seen"}}},
            {"edit": {"item": "clicked", "toggle": true}},
            {"edit": {"recipe": "rename-tag", "options": {"from": "a", "to": "b"}}},
            {"create": {"text": "# New"}},
            {"rebind": {"shape": "card", "inputs": {"note": "created"}}},
        ]));
        assert_eq!(Steps::checked(good.clone()).as_deref(), Ok(&good[..]));
        assert_eq!(Steps::checked(Vec::new()), Ok(Steps::default()));

        let bad = steps(json!([
            {"edit": {"item": "clicked", "set-field": {"field": "visited"}}},
            {"create": {"text": "# New"}},
            {"edit": {"options": {"from": "a"}}},
            {"edit": {"item": "clicked", "recipe": "rename-tag", "toggle": true}},
            {"edit": {"item": "clicked", "set-field": {"field": "tags", "add": "a", "remove": "b"}}},
        ]));
        assert_eq!(
            Steps::checked(bad),
            Err(vec![
                "set-field takes exactly one of value, add and remove".to_string(),
                "an edit is exactly one of set-field, toggle and recipe".to_string(),
                "say which item set-field or toggle changes, like item = \"dropped\"".to_string(),
                "options are a recipe's".to_string(),
                "an edit is exactly one of set-field, toggle and recipe".to_string(),
                "a recipe changes the whole workspace, so it names no item".to_string(),
                "set-field takes exactly one of value, add and remove".to_string(),
            ])
        );
    }
}
