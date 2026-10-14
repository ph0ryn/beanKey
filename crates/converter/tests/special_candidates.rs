use beankey_converter::{ComposingText, InputStyle, InputTableRegistry, special_candidates};

#[test]
fn converts_meiji_years_and_transition_boundaries() {
    let tables = InputTableRegistry::new();
    for (input, expected) in [
        ("1868ねん", vec!["明治元年", "慶應4年"]),
        ("1869ねん", vec!["明治2年"]),
        ("1900ねん", vec!["明治33年"]),
        ("1911ねん", vec!["明治44年"]),
        ("1912ねん", vec!["大正元年", "明治45年"]),
    ] {
        let mut composing = ComposingText::new();
        composing.insert_str(input, InputStyle::Direct, &tables);
        let actual: Vec<_> = special_candidates(&composing, None)
            .into_iter()
            .map(|candidate| candidate.text)
            .collect();
        assert_eq!(actual, expected, "era candidates for {input}");
    }
}
