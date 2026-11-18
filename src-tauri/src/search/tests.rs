//! Search tests. Items are built in memory; nothing here touches disk.

use super::*;

#[test]
fn blank_queries_have_no_terms() {
    assert!(Query::parse("").is_none());
    assert!(Query::parse("   \t ").is_none());
    // A lone operator has no needle either.
    assert!(Query::parse("!").is_none());
}

#[test]
fn plain_words_become_fuzzy_terms_with_an_exact_twin() {
    let q = Query::parse("foo bar").unwrap();
    assert_eq!(q.terms.len(), 2);
    assert_eq!(q.positives, 2);
    for t in &q.terms {
        assert_eq!(t.atom.kind, AtomKind::Fuzzy);
        assert_eq!(t.exact.as_ref().unwrap().kind, AtomKind::Substring);
    }
}

#[test]
fn operators_are_parsed_and_have_no_twin() {
    let q = Query::parse("^foo bar$ 'baz !qux").unwrap();
    let kinds: Vec<_> = q.terms.iter().map(|t| t.atom.kind).collect();
    assert_eq!(
        kinds,
        [
            AtomKind::Prefix,
            AtomKind::Postfix,
            AtomKind::Substring,
            AtomKind::Substring
        ]
    );
    assert!(q.terms.iter().all(|t| t.exact.is_none()));
    assert_eq!(q.positives, 3, "the negated word does not have to match");
}

#[test]
fn the_matcher_is_reused_on_a_thread() {
    // The scratch buffers keep their allocation between calls.
    with_scratch(|s| s.chars.extend(['a'; 64]));
    with_scratch(|s| assert!(s.chars.capacity() >= 64));
}

#[test]
fn tabs_and_newlines_separate_words() {
    let q = Query::parse("foo\tbar\nbaz").unwrap();
    assert_eq!(q.terms.len(), 3);
}
