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

// -- filters -------------------------------------------------------------

#[test]
fn prefixes_are_stripped_and_set_the_filter() {
    let p = parse_query("img: cat photo", TypeFilter::All);
    assert_eq!(p.text, "cat photo");
    assert_eq!(p.filter, TypeFilter::Images);
    assert_eq!(
        parse_query("file:report", TypeFilter::All).filter,
        TypeFilter::Files
    );
    assert_eq!(parse_query("file:report", TypeFilter::All).text, "report");
    assert_eq!(parse_query("txt:  hi", TypeFilter::All).text, "hi");
    assert_eq!(
        parse_query("txt:hi", TypeFilter::All).filter,
        TypeFilter::Text
    );
}

#[test]
fn a_typed_prefix_overrides_the_ui_filter() {
    for ui in [
        TypeFilter::All,
        TypeFilter::Text,
        TypeFilter::Images,
        TypeFilter::Files,
    ] {
        assert_eq!(parse_query("img:x", ui).filter, TypeFilter::Images);
        assert_eq!(parse_query("txt:x", ui).filter, TypeFilter::Text);
    }
}

#[test]
fn without_a_prefix_the_ui_filter_applies() {
    let p = parse_query("  hello img:", TypeFilter::Files);
    assert_eq!(p.filter, TypeFilter::Files);
    assert_eq!(p.text, "hello img:", "only a leading prefix counts");
}

#[test]
fn prefixes_are_case_insensitive_and_alone_is_valid() {
    let p = parse_query("  IMG:", TypeFilter::Text);
    assert_eq!(p.filter, TypeFilter::Images);
    assert_eq!(p.text, "");
    assert_eq!(
        parse_query("Txt:a", TypeFilter::All).filter,
        TypeFilter::Text
    );
}

#[test]
fn similar_words_are_not_prefixes() {
    assert_eq!(
        parse_query("image:x", TypeFilter::All).filter,
        TypeFilter::All
    );
    assert_eq!(parse_query("im", TypeFilter::All).text, "im");
    // Multibyte text right at the cut point must not panic.
    assert_eq!(parse_query("ééé", TypeFilter::All).text, "ééé");
}

#[test]
fn type_filter_accepts_the_right_kinds() {
    use crate::model::ClipKind::*;
    assert!(TypeFilter::All.accepts(Text) && TypeFilter::All.accepts(Image));
    assert!(TypeFilter::Text.accepts(Text) && !TypeFilter::Text.accepts(Image));
    assert!(TypeFilter::Images.accepts(Image) && !TypeFilter::Images.accepts(Files));
    assert!(TypeFilter::Files.accepts(Files) && !TypeFilter::Files.accepts(Text));
}

#[test]
fn type_filter_deserializes_from_the_ui_names() {
    let f: TypeFilter = serde_json::from_str("\"images\"").unwrap();
    assert_eq!(f, TypeFilter::Images);
    assert_eq!(
        serde_json::from_str::<TypeFilter>("\"all\"").unwrap(),
        TypeFilter::All
    );
}
