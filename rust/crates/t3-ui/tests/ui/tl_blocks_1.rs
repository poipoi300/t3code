//! Upstream-generated splitter cases; regeneration is recorded in reports/ui-tl-blocks-1.md.

use t3_ui::screens::thread::blocks::flatten::split_markdown_blocks;

#[test]
fn upstream_splitter_fixtures() {
    for &(source, expected) in SPLITTER_FIXTURES {
        assert_eq!(split_markdown_blocks(source), expected.map(<[_]>::to_vec), "{source:?}");
    }
}

const SPLITTER_FIXTURES: &[(&str, Option<&[&str]>)] = &[
    ("", Some(&[""])),
    ("a\n\nb", Some(&["a\n\n", "b"])),
    ("a\n\n```\nb\n\nc\n`", Some(&["a\n\n", "```\nb\n\nc\n`"])),
    ("a\n\n```\nb\n\nc\n```\n\n- d\n\n- e\n", Some(&["a\n\n", "```\nb\n\nc\n```\n\n- d\n\n- e\n"])),
    ("a\n\n```\nb\n\nc\n```\n\n- d\n\n- e\n\nf", Some(&["a\n\n", "```\nb\n\nc\n```\n\n- d\n\n- e\n\n", "f"])),
    ("# Title\n\nFirs", Some(&["# Title\n\n", "Firs"])),
    ("# Title\n\nFirst paragraph w", Some(&["# Title\n\n", "First paragraph w"])),
    ("# Title\n\nFirst paragraph with `code` an", Some(&["# Title\n\n", "First paragraph with `code` an"])),
    ("# Title\n\nFirst paragraph with `code` and **bold**.\n\n", Some(&["# Title\n\n", "First paragraph with `code` and **bold**.\n\n"])),
    ("# Title\n\nFirst paragraph with `code` and **bold**.\n\nSecond [link]", Some(&["# Title\n\n", "First paragraph with `code` and **bold**.\n\n", "Second [link]"])),
    ("# Title\n\nFirst paragraph with `code` and **bold**.\n\nSecond [link](https://exam", Some(&["# Title\n\n", "First paragraph with `code` and **bold**.\n\n", "Second [link](https://exam"])),
    ("# Title\n\nFirst paragraph with `code` and **bold**.\n\nSecond [link](https://example.com).", Some(&["# Title\n\n", "First paragraph with `code` and **bold**.\n\n", "Second [link](https://example.com)."])),
    ("- one\n- two\n\n", Some(&["- one\n- two\n\n"])),
    ("- one\n- two\n\n- loose three", Some(&["- one\n- two\n\n- loose three"])),
    ("- one\n- two\n\n- loose three\n\n  continued", Some(&["- one\n- two\n\n- loose three\n\n  continued"])),
    ("- one\n- two\n\n- loose three\n\n  continued\n\nAfter the l", Some(&["- one\n- two\n\n- loose three\n\n  continued\n\n", "After the l"])),
    ("- one\n- two\n\n- loose three\n\n  continued\n\nAfter the list.", Some(&["- one\n- two\n\n- loose three\n\n  continued\n\n", "After the list."])),
    ("1. first\n\n2. ", Some(&["1. first\n\n2. "])),
    ("1. first\n\n2. second\n\nText\n", Some(&["1. first\n\n2. second\n\n", "Text\n"])),
    ("1. first\n\n2. second\n\nText\n\n3) restarted", Some(&["1. first\n\n2. second\n\n", "Text\n\n3) restarted"])),
    ("```ts\nconst a", Some(&["```ts\nconst a"])),
    ("```ts\nconst a = 1;\n\n\nconst", Some(&["```ts\nconst a = 1;\n\n\nconst"])),
    ("```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\n", Some(&["```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\n"])),
    ("```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\nafter\n\n~~~\nx\n", Some(&["```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\n", "after\n\n", "~~~\nx\n"])),
    ("```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\nafter\n\n~~~\nx\n\n~~~~\n\ny\n~~~\n", Some(&["```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\n", "after\n\n", "~~~\nx\n\n~~~~\n\n", "y\n~~~\n"])),
    ("```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\nafter\n\n~~~\nx\n\n~~~~\n\ny\n~~~\n\nend", Some(&["```ts\nconst a = 1;\n\n\nconst b = 2;\n```\n\n", "after\n\n", "~~~\nx\n\n~~~~\n\n", "y\n~~~\n\nend"])),
    ("> quote\n\n> an", Some(&["> quote\n\n", "> an"])),
    ("> quote\n\n> another quote\n\n", Some(&["> quote\n\n", "> another quote\n\n"])),
    ("> quote\n\n> another quote\n\n> [!NOTE]\n> a", Some(&["> quote\n\n", "> another quote\n\n", "> [!NOTE]\n> a"])),
    ("> quote\n\n> another quote\n\n> [!NOTE]\n> alert", Some(&["> quote\n\n", "> another quote\n\n", "> [!NOTE]\n> alert"])),
    ("| a | b |\n| -", Some(&["| a | b |\n| -"])),
    ("| a | b |\n| - | - |\n| 1 | ", Some(&["| a | b |\n| - | - |\n| 1 | "])),
    ("| a | b |\n| - | - |\n| 1 | 2 |\n\nAfter th", Some(&["| a | b |\n| - | - |\n| 1 | 2 |\n\n", "After th"])),
    ("| a | b |\n| - | - |\n| 1 | 2 |\n\nAfter the table.", Some(&["| a | b |\n| - | - |\n| 1 | 2 |\n\n", "After the table."])),
    ("para\n\n    ind", Some(&["para\n\n    ind"])),
    ("para\n\n    indented code\n\nn", Some(&["para\n\n    indented code\n\n", "n"])),
    ("para\n\n    indented code\n\nnext", Some(&["para\n\n    indented code\n\n", "next"])),
    ("Setext\n===\n\nn", Some(&["Setext\n===\n\n", "n"])),
    ("Setext\n===\n\nnext\n---\n\n- [ ", Some(&["Setext\n===\n\n", "next\n---\n\n- [ "])),
    ("Setext\n===\n\nnext\n---\n\n- [ ] task\n- [x] ", Some(&["Setext\n===\n\n", "next\n---\n\n- [ ] task\n- [x] "])),
    ("Setext\n===\n\nnext\n---\n\n- [ ] task\n- [x] done", Some(&["Setext\n===\n\n", "next\n---\n\n- [ ] task\n- [x] done"])),
    ("- a\n\n  ```\n  ", Some(&["- a\n\n  ```\n  "])),
    ("- a\n\n  ```\n  x\n\n```\n\nb", Some(&["- a\n\n  ```\n  x\n\n```\n\nb"])),
    ("- a\n\n  ```\n  x\n\ny\n\n  ```\n\n", Some(&["- a\n\n  ```\n  x\n\ny\n\n  ```\n\n"])),
    ("- a\n\n  ```\n  x\n\ny\n\n  ```\n\nb", Some(&["- a\n\n  ```\n  x\n\ny\n\n  ```\n\nb"])),
    ("intro\n\n★ Insi", Some(&["intro\n\n", "★ Insi"])),
    ("intro\n\n★ Insight ─────\nfir", Some(&["intro\n\n", "★ Insight ─────\nfir"])),
    ("intro\n\n★ Insight ─────\nfirst\n\n```\n─────", Some(&["intro\n\n", "★ Insight ─────\nfirst\n\n```\n─────"])),
    ("intro\n\n★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point", Some(&["intro\n\n", "★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point"])),
    ("intro\n\n★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point\n─────\n\nafter", Some(&["intro\n\n", "★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point\n─────\n\n", "after"])),
    ("intro\n\n★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point\n─────\n\nafter\n\n`★ Insight ", Some(&["intro\n\n", "★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point\n─────\n\n", "after\n\n", "`★ Insight "])),
    ("intro\n\n★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point\n─────\n\nafter\n\n`★ Insight ─────`\nsecond", Some(&["intro\n\n", "★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point\n─────\n\n", "after\n\n", "`★ Insight ─────`\nsecond"])),
    ("intro\n\n★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point\n─────\n\nafter\n\n`★ Insight ─────`\nsecond\n`─────`\n\nend", Some(&["intro\n\n", "★ Insight ─────\nfirst\n\n```\n─────\n```\n\n- point\n─────\n\n", "after\n\n", "`★ Insight ─────`\nsecond\n`─────`\n\n", "end"])),
    ("[ref]\n\n[ref]:", None),
    ("[ref]\n\n[ref]: /later", None),
    ("a[^x]\n\n[^x]: ", None),
    ("a[^x]\n\n[^x]: note", None),
    ("<details>\n\nbo", None),
    ("<details>\n\nbody\n\n</details", None),
    ("<details>\n\nbody\n\n</details>", None),
    (":::note\n\ncont", None),
    (":::note\n\ncontent\n\n:::", None),
    ("$$\na\n\nb\n$$", None),
    ("a\r\n\r\nb", None),
    ("`★ Insight ──", Some(&["`★ Insight ──"])),
    ("`★ Insight ─── `\ntext\n\naft", Some(&["`★ Insight ─── `\ntext\n\n", "aft"])),
    ("`★ Insight ─── `\ntext\n\nafter", Some(&["`★ Insight ─── `\ntext\n\n", "after"])),
    ("`★ Insight ───`\ntext\n`─── ", Some(&["`★ Insight ───`\ntext\n`─── "])),
    ("`★ Insight ───`\ntext\n`─── `\n\nafter", Some(&["`★ Insight ───`\ntext\n`─── `\n\nafter"])),
    ("text $$", None),
    ("text <details", None),
    ("text <details>body</detail", None),
    ("text <details>body</details>", None),
    ("[label contin", Some(&["[label contin"])),
    ("[label continued]: /target", None),
    ("😀\n\n```rs\n😀\n", Some(&["😀\n\n", "```rs\n😀\n"])),
    ("😀\n\n```rs\n😀\n```\n\nnext", Some(&["😀\n\n", "```rs\n😀\n```\n\n", "next"])),
    ("﻿paragraph\n\nn", Some(&["﻿paragraph\n\n", "n"])),
    ("﻿paragraph\n\nnext", Some(&["﻿paragraph\n\n", "next"])),
];
