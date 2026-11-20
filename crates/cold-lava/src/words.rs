//! The app's own words that change with a number.

/// `n` and `word`, with an `s` unless `n` is 1: "1 note", "0 notes", "3 notes".
pub fn plural(n: usize, word: &str) -> String {
    match n {
        1 => format!("1 {word}"),
        n => format!("{n} {word}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_is_singular_and_every_other_count_takes_an_s() {
        assert_eq!(plural(0, "note"), "0 notes");
        assert_eq!(plural(1, "note"), "1 note");
        assert_eq!(plural(2, "other file"), "2 other files");
    }
}
