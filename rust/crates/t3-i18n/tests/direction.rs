//! Ports of `packages/shared` / `apps/web/src/textDirection.test.ts` and the block rules of
//! `composer-text-direction.test.ts`.

use t3_i18n::{
    DirectionBlock,
    TextDirection::{Ltr, Rtl},
    composer_block_directions, markdown_prose_direction, text_direction,
};

#[test]
fn text_direction_cases() {
    let cases = [
        ("Arabic", "اكتب دالة تجمع الأرقام", Some(Rtl)),
        ("Hebrew", "תקן את הבאג", Some(Rtl)),
        ("English", "fix the failing test", Some(Ltr)),
        (
            "Arabic that opens with an English term",
            "StrictMode: يعيد الـ render مرتين في التطوير",
            Some(Rtl),
        ),
        (
            "English quoting one Arabic word",
            "translate the label مرحبا for the login screen",
            Some(Ltr),
        ),
        (
            "tied sentences use the whole-text word majority",
            "Done. تم إصلاح المشكلة في الملف الرئيسي",
            Some(Rtl),
        ),
        ("a tie, by its first word", "React مكتبة", Some(Ltr)),
        ("a tie that opens in Arabic", "مكتبة React", Some(Rtl)),
        (
            "Arabic around numbers and punctuation",
            "عدّل السطر 42، ثم شغّل الاختبارات (3)",
            Some(Rtl),
        ),
        (
            "mostly Arabic sentences beside one long English one",
            "هذا مثال على paragraph يحتوي على نص عربي و English words معًا. Arabic text should flow right-to-left while English words like JavaScript, React, and API should remain left-to-right within the same line. الأرقام 1234567890 ورموز مثل @username و #hashtag و 100$ تظهر في الاتجاه الصحيح. هذه جملة لاختبار RTL rendering في المتصفح أو المحرر.",
            Some(Rtl),
        ),
        (
            "mostly English sentences beside one Arabic one",
            "Run the migration first. ثم شغّل الاختبارات كلها. Then deploy to staging.",
            Some(Ltr),
        ),
        (
            "Arabic with inline abbreviations",
            "هذه جملة عربية تتحدث عن e.g. React i.e. TypeScript JavaScript",
            Some(Rtl),
        ),
        (
            "an abbreviation at a sentence boundary",
            "This long English sentence describes the technology sector in the U.S. هذه جملة عربية. وهذه جملة عربية أخرى.",
            Some(Rtl),
        ),
        ("without letters", "42 — 7 ✓", None),
        ("empty", "", None),
    ];
    for (name, text, expected) in cases {
        assert_eq!(text_direction(text), expected, "{name}");
    }
}

#[test]
fn markdown_prose_direction_cases() {
    let cases = [
        (
            "Hebrew around a code block",
            "תתקן את זה:\n```ts\nexport function useCheckout(props: CheckoutProps) {\n  return props.items.length;\n}\n```\nותריץ שוב",
            Some(Rtl),
        ),
        (
            "Hebrew around inline code",
            "לערוך את `apps/web/src/chat.tsx` או להשאיר `useMemo`?",
            Some(Rtl),
        ),
        (
            "Hebrew around double-backtick code",
            "תבדוק ``const a = `b c d e` + f`` עכשיו",
            Some(Rtl),
        ),
        (
            "Hebrew after a four-backtick fence",
            "````md\none two three four five six\n````\nשלום עולם, זה עובד",
            Some(Rtl),
        ),
        (
            "Hebrew after a tilde fence",
            "~~~\nnpm run build --filter web --force\n~~~\nזה נכשל אצלי",
            Some(Rtl),
        ),
        (
            "Hebrew before an unclosed fence",
            "שלום, הנה השגיאה:\n```\nError: cannot find module react dom client",
            Some(Rtl),
        ),
        (
            "Hebrew around a URL",
            "ראה https://github.com/pingdotgg/t3code/pull/24050 לפני",
            Some(Rtl),
        ),
        (
            "English around inline code",
            "Should I edit `useMemo` or keep it?",
            Some(Ltr),
        ),
        (
            "English quoting one Hebrew word",
            "Please check the שלום label before the release",
            Some(Ltr),
        ),
        ("no prose letters", "```\nconst a = 1;\n```", None),
    ];
    for (name, markdown, expected) in cases {
        assert_eq!(markdown_prose_direction(markdown), expected, "{name}");
    }
}

#[test]
fn regex_edge_cases_follow_javascript() {
    // `\s` includes U+FEFF and U+00A0; a sentence break needs whitespace after the punctuation.
    assert_eq!(
        text_direction("Done.\u{00A0}تم إصلاح المشكلة في الملف الرئيسي"),
        Some(Rtl)
    );
    // `e.g.`/`i.e.` only suppress a break when they start a word, and match case-insensitively.
    assert_eq!(
        text_direction("A B C. D مرحبا"),
        text_direction("A B C.\nD مرحبا")
    );
    // An abbreviation inside a longer word still ends a sentence.
    assert_eq!(text_direction("name.e.g. مرحبا بالعالم الجميل"), Some(Rtl));
    // An inline code span needs a closing run of the same length that no backtick touches.
    assert_eq!(
        markdown_prose_direction("`one two three four `` שלום"),
        Some(Ltr)
    );
    // An unmatched single backtick stays prose.
    assert_eq!(markdown_prose_direction("`a b c d e f g שלום"), Some(Ltr));
}

#[test]
fn composer_block_rules() {
    let blocks = |items: &[(&'static str, bool)]| {
        let blocks: Vec<_> = items
            .iter()
            .map(|(text, is_code)| DirectionBlock {
                text,
                is_code: *is_code,
            })
            .collect();
        composer_block_directions(&blocks)
    };
    // "continues the line above until a line has letters"
    assert_eq!(
        blocks(&[
            ("اكتب الدالة", false),
            ("", false),
            ("42", false),
            ("fix it", false)
        ]),
        vec![Some(Rtl), Some(Rtl), Some(Rtl), Some(Ltr)]
    );
    // "keeps a code block left to right inside Arabic text"
    assert_eq!(
        blocks(&[
            ("اكتب الدالة", false),
            ("// احسب مجموع القيم هنا", true),
            ("ثم اختبرها", false)
        ]),
        vec![Some(Rtl), Some(Ltr), Some(Rtl)]
    );
    assert_eq!(blocks(&[("42", false)]), vec![None]);
}

/// 1,500 generated strings (code spans, fences, URLs, abbreviations, mixed scripts, odd
/// whitespace) with the answers of the real TypeScript functions, from
/// `tests/fixtures/direction.json` (regenerate with `tools/i18n-fixtures`, see the README).
#[test]
fn matches_the_typescript_on_generated_text() {
    #[derive(serde::Deserialize)]
    struct Case {
        text: String,
        direction: Option<String>,
        markdown: Option<String>,
    }
    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/direction.json")).expect("fixture");
    let name = |direction: Option<t3_i18n::TextDirection>| direction.map(|d| d.as_str().to_owned());
    let mut mismatches = Vec::new();
    for case in &cases {
        if name(text_direction(&case.text)) != case.direction
            || name(markdown_prose_direction(&case.text)) != case.markdown
        {
            mismatches.push(case.text.clone());
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} differ, first: {:?}",
        mismatches.len(),
        cases.len(),
        mismatches.first()
    );
}
