use super::*;

fn kinds(src: &str) -> Vec<(TokenKind, String)> {
    tokenize(src)
        .expect("lexing succeeds")
        .into_iter()
        .filter(|t| t.kind != TokenKind::Eof)
        .map(|t| (t.kind, t.text))
        .collect()
}

#[test]
fn range_between_integers_is_not_a_float() {
    assert_eq!(
        kinds("0..10"),
        vec![
            (TokenKind::Number, "0".to_string()),
            (TokenKind::DotDot, "..".to_string()),
            (TokenKind::Number, "10".to_string()),
        ]
    );
}

#[test]
fn float_literal_keeps_its_dot() {
    assert_eq!(kinds("3.14"), vec![(TokenKind::Number, "3.14".to_string())]);
}

#[test]
fn tuple_field_access_on_number_splits_the_dot() {
    assert_eq!(
        kinds("3.foo"),
        vec![
            (TokenKind::Number, "3".to_string()),
            (TokenKind::Dot, ".".to_string()),
            (TokenKind::Identifier, "foo".to_string()),
        ]
    );
}

#[test]
fn inclusive_range_fuses_regardless_of_spacing() {
    // Digit-adjacent, where the number first swallows a dot, and cleanly spaced
    // both recover a single `..=`.
    let expected = |lo: &str, hi: &str| {
        vec![
            (TokenKind::Number, lo.to_string()),
            (TokenKind::DotDotEquals, "..=".to_string()),
            (TokenKind::Number, hi.to_string()),
        ]
    };
    assert_eq!(kinds("0..=10"), expected("0", "10"));
    assert_eq!(kinds("0 ..= 10"), expected("0", "10"));
}
