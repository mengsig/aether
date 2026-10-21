#![cfg(test)]

use super::*;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    widgets::{Paragraph, Widget},
};

use crate::ui::theme::ThemeSpec;

fn text_of(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn widths_ok(lines: &[Line<'_>], width: usize) {
    for l in lines {
        assert!(
            text_of(l).width() <= width,
            "{:?} is wider than {width}",
            text_of(l)
        );
    }
}

#[test]
fn wrapped_rows_rejoin_into_their_source_lines_at_any_width() {
    let theme = Theme::resolve(ThemeSpec::default());
    let reply = "A paragraph  with words,\nsoft breaks and averyveryverylongwordindeed.\n\n\
        > quoted text that wraps over rows\n\n\
        - a list item long enough to wrap\n  - nested item that wraps as well\n\n\
        ```\nlet code = \"a long line of code that splits at the width\";\n```";
    let base = theme.text();
    let wide = render_layout(reply, 1_000, base, &theme).rejoined();
    assert!(
        wide.contains(
            &"A paragraph  with words, soft breaks and averyveryverylongwordindeed.".into()
        )
    );
    for width in 12..60 {
        let narrow = render_layout(reply, width, base, &theme);
        assert_eq!(narrow.rejoined(), wide, "width {width}");
    }
}

#[test]
fn paragraphs_wrap_and_blocks_are_separated() {
    let lines = render(
        "First paragraph with enough words to need wrapping twice over.\n\nSecond.",
        24,
        Style::default(),
        &Theme::default(),
    );
    widths_ok(&lines, 24);
    let texts: Vec<String> = lines.iter().map(text_of).collect();
    assert_eq!(texts[0], "First paragraph with");
    assert!(texts.contains(&String::new()), "blank line between blocks");
    assert_eq!(texts.last().map(String::as_str), Some("Second."));
}

#[test]
fn lists_get_secondary_markers_and_hanging_indents() {
    let theme = Theme::default();
    let lines = render(
        "- alpha beta gamma delta epsilon\n- b\n  - nested\n\n1. one\n2. two",
        20,
        Style::default(),
        &theme,
    );
    let texts: Vec<String> = lines.iter().map(text_of).collect();
    assert_eq!(texts[0], "- alpha beta gamma");
    assert_eq!(texts[1], "  delta epsilon");
    assert_eq!(texts[2], "- b");
    assert_eq!(texts[3], "  - nested");
    assert!(texts.contains(&"1. one".to_owned()), "{texts:?}");
    assert!(texts.contains(&"2. two".to_owned()), "{texts:?}");
    assert_eq!(lines[0].spans[0].content, "- ");
    assert_eq!(lines[0].spans[0].style, theme.secondary());
    let numbered = lines
        .iter()
        .find(|l| text_of(l) == "1. one")
        .expect("ordered item");
    assert_eq!(numbered.spans[0].content, "1. ");
    assert_eq!(numbered.spans[0].style, theme.secondary());
    widths_ok(&lines, 20);
}

#[test]
fn fenced_code_is_unboxed_and_indented_and_code_spans_are_accent() {
    let theme = Theme::default();
    let lines = render(
        "Run `cargo test` now:\n\n```rust\nfn main() {}\n\tlet x = 1;\n```\nafter",
        30,
        Style::default(),
        &theme,
    );
    let inline = lines[0]
        .spans
        .iter()
        .find(|s| s.content == "cargo test")
        .expect("inline code span");
    assert_eq!(inline.style, theme.code());
    assert!(
        lines.iter().all(|l| text_of(l) != "rust"),
        "no bare language line: {lines:?}"
    );
    let code = lines
        .iter()
        .find(|l| text_of(l).contains("fn main() {}"))
        .expect("code line");
    assert_eq!(text_of(code), "  fn main() {}", "no box, two columns in");
    assert!(code.spans.iter().all(|s| s.style == Style::default()));
    assert!(
        lines
            .iter()
            .all(|l| l.spans.iter().all(|s| s.style.bg.is_none())),
        "nothing paints a background"
    );
    assert!(lines.iter().any(|l| text_of(l) == "      let x = 1;"));
    assert_eq!(text_of(lines.last().expect("after")), "after");
}

#[test]
fn unterminated_fence_while_streaming_is_still_code() {
    let lines = render(
        "Here:\n\n```sh\n# partial line",
        40,
        Style::default(),
        &Theme::default(),
    );
    let texts: Vec<String> = lines.iter().map(text_of).collect();
    assert_eq!(texts, ["Here:", "", "  # partial line"]);
}

#[test]
fn headings_follow_the_spec_weights() {
    let theme = Theme::default();
    let lines = render("# One\n\n## Two\n\n### Three", 40, Style::default(), &theme);
    let style_of = |text: &str| {
        lines
            .iter()
            .find(|l| text_of(l) == text)
            .unwrap_or_else(|| panic!("no {text:?} heading"))
            .spans[0]
            .style
    };
    assert_eq!(style_of("One"), theme.heading().patch(theme.underline()));
    assert_eq!(style_of("Two"), theme.heading());
    assert_eq!(style_of("Three"), theme.heading().patch(theme.emphasis()));
    let one = style_of("One").add_modifier;
    assert!(one.contains(Modifier::BOLD) && one.contains(Modifier::UNDERLINED));
    assert!(style_of("Three").add_modifier.contains(Modifier::ITALIC));
}

#[test]
fn emphasis_links_and_quotes() {
    let theme = Theme::default();
    let lines = render(
        "> quoted *words* here\n\nSee [docs](https://x.y) and **bold**  \nnext line",
        40,
        Style::default(),
        &theme,
    );
    let texts: Vec<String> = lines.iter().map(text_of).collect();
    let quote = lines
        .iter()
        .find(|l| text_of(l).starts_with("▎ "))
        .expect("quote");
    assert_eq!(quote.spans[0].style, theme.secondary());
    let italic = quote
        .spans
        .iter()
        .find(|s| s.content == "words")
        .expect("italic run");
    assert!(italic.style.add_modifier.contains(Modifier::ITALIC));
    let link_line = lines
        .iter()
        .find(|l| text_of(l).contains("docs"))
        .expect("link");
    let link = link_line
        .spans
        .iter()
        .find(|s| s.content == "docs")
        .expect("link text");
    assert_eq!(link.style, theme.link());
    let url = link_line
        .spans
        .iter()
        .find(|s| s.content == " (https://x.y)")
        .expect("url after the link");
    assert_eq!(url.style, theme.secondary());
    let bold = link_line
        .spans
        .iter()
        .find(|s| s.content == "bold")
        .expect("bold run");
    assert!(bold.style.add_modifier.contains(Modifier::BOLD));
    assert!(
        texts.contains(&"next line".to_owned()),
        "hard break: {texts:?}"
    );
}

#[test]
fn ascii_quotes_use_a_pipe() {
    let theme = Theme::resolve(ThemeSpec {
        ascii: true,
        ..ThemeSpec::default()
    });
    let lines = render("> quoted", 40, Style::default(), &theme);
    assert_eq!(text_of(&lines[0]), "| quoted");
}

#[test]
fn tables_align_columns() {
    let lines = render(
        "| name | cost |\n|---|---|\n| opus | $5 |\n| sonnet | $2 |",
        40,
        Style::default(),
        &Theme::default(),
    );
    let texts: Vec<String> = lines.iter().map(text_of).collect();
    assert_eq!(texts[0].trim_end(), "name    cost");
    assert!(texts[1].starts_with("────"));
    assert_eq!(texts[2].trim_end(), "opus    $5");
    assert_eq!(texts[3].trim_end(), "sonnet  $2");
}

#[test]
fn wide_tables_stay_inside_narrow_chat_panes() {
    let lines = render(
        "| a | b | c | d | e |\n|---|---|---|---|---|\n| alpha | beta | gamma | delta | epsilon |",
        8,
        Style::default(),
        &Theme::default(),
    );
    widths_ok(&lines, 8);
    assert!(lines.iter().any(|line| text_of(line).contains('a')));
}

#[test]
fn long_table_cells_wrap_at_every_width() {
    let md = "| step | detail |\n|---|---|\n| one | Run the full verification suite before opening the pull request |\n| two | short |";
    for width in [60, 40, 24, 16, 10, 8] {
        let lines = render(md, width, Style::default(), &Theme::default());
        widths_ok(&lines, width);
        let joined: String = lines.iter().map(text_of).collect::<Vec<_>>().join(" ");
        if width >= 24 {
            for word in ["verification", "pull", "request"] {
                assert!(joined.contains(word), "{word} lost at width {width}");
            }
        }
    }
    let lines = render(md, 30, Style::default(), &Theme::default());
    let texts: Vec<String> = lines.iter().map(text_of).collect();
    assert!(texts.len() > 5, "cell did not wrap: {texts:?}");
    let second = texts.iter().position(|t| t.starts_with("two")).unwrap();
    assert!(
        texts[second - 1].starts_with("    "),
        "continuation not aligned"
    );
}

#[test]
fn long_words_are_split_instead_of_overflowing() {
    let lines = render(
        "https://example.com/a/very/long/path/that/never/ends/at/all",
        16,
        Style::default(),
        &Theme::default(),
    );
    widths_ok(&lines, 16);
    assert!(lines.len() >= 3);
}

#[test]
fn long_words_wrap_at_grapheme_boundaries() {
    for cluster in ["👩‍💻", "🇩🇰", "👍🏽", "e\u{301}"] {
        let prefix = "a".repeat(8 - cluster.width());
        let text = format!("{prefix}{cluster}X");
        let lines = render(&text, 8, Style::default(), &Theme::default());
        assert_eq!(
            lines.iter().map(text_of).collect::<Vec<_>>(),
            [format!("{prefix}{cluster}"), "X".to_owned()],
            "{cluster:?}"
        );
        widths_ok(&lines, 8);
    }
}

#[test]
fn fenced_code_wraps_at_grapheme_boundaries() {
    let lines = render(
        "```\nabcdef👩‍💻X\n```",
        10,
        Style::default(),
        &Theme::default(),
    );
    assert_eq!(
        lines.iter().map(text_of).collect::<Vec<_>>(),
        ["  abcdef👩‍💻", "  X"]
    );
    widths_ok(&lines, 10);
}

#[test]
fn graphemes_crossing_inline_styles_use_the_leading_style() {
    let theme = Theme::default();
    for (text, expected) in [
        ("abcdef`👩`‍💻X", vec!["abcdef👩‍💻", "X"]),
        ("ab`👩`‍💻 cd", vec!["ab👩‍💻 cd"]),
        ("abcdefg`e`\u{301}X", vec!["abcdefge\u{301}", "X"]),
    ] {
        let lines = render(text, 8, Style::default(), &theme);
        assert_eq!(lines.iter().map(text_of).collect::<Vec<_>>(), expected);
        widths_ok(&lines, 8);
        for line in &lines {
            assert_eq!(line.width(), text_of(line).width());
        }
        let cluster = lines[0]
            .spans
            .iter()
            .find(|s| s.content == "👩‍💻" || s.content == "e\u{301}")
            .expect("one complete styled grapheme");
        assert_eq!(cluster.style, theme.code());
        assert_eq!(
            lines.last().unwrap().spans.last().unwrap().style,
            Style::default()
        );
    }
}

#[test]
fn wrapping_matches_terminal_cells_after_style_normalization() {
    for (text, first, last) in [
        ("abcdefل`\u{64e}`اX", "abcdefل\u{64e}ا", "ا"),
        ("abcdefｶ`ﾞ`X", "abcdefｶﾞ", " "),
    ] {
        let lines = render(text, 8, Style::default(), &Theme::default());
        assert_eq!(lines.iter().map(text_of).collect::<Vec<_>>(), [first, "X"]);
        let area = Rect::new(0, 0, 8, 2);
        let mut buffer = Buffer::empty(area);
        Paragraph::new(lines).render(area, &mut buffer);
        assert_eq!(buffer[(7, 0)].symbol(), last);
        assert_eq!(buffer[(0, 1)].symbol(), "X");
    }
}

#[test]
fn fenced_code_and_tables_use_terminal_cell_widths() {
    let theme = Theme::default();
    let code = render("```\nabcdefｶﾞX\n```", 10, Style::default(), &theme);
    assert_eq!(
        code.iter().map(text_of).collect::<Vec<_>>(),
        ["  abcdefｶﾞ", "  X"]
    );
    assert_eq!(display_width(&text_of(&code[0])), 10);

    let table = render("| cell |\n|---|\n| abcdefｶﾞX |", 8, Style::default(), &theme);
    assert_eq!(text_of(&table[2]), "abcdefｶﾞ");
    assert_eq!(display_width(&text_of(&table[2])), 8);
    let area = Rect::new(0, 0, 8, 3);
    let mut buffer = Buffer::empty(area);
    Paragraph::new(table).render(area, &mut buffer);
    assert_eq!(buffer[(6, 2)].symbol(), "ｶﾞ");
}

#[test]
fn oversized_graphemes_remain_in_paragraph_and_code_output() {
    let cluster = "\u{1100}".repeat(5);
    let theme = Theme::default();
    let lines = render(&format!("a{cluster}X"), 8, Style::default(), &theme);
    assert_eq!(
        lines.iter().map(text_of).collect::<Vec<_>>(),
        ["a", &cluster, "X"]
    );
    let code = render(
        &format!("```\n{cluster}\n```"),
        10,
        Style::default(),
        &theme,
    );
    assert_eq!(text_of(&code[0]), format!("  {cluster}"));
}

#[test]
fn ordered_list_marker_width_grows_with_the_item_number() {
    let theme = Theme::default();
    let text = (1..=12)
        .map(|n| format!("{n}. abcde fghij klmno pqrst\n"))
        .collect::<String>();
    let layout = render_layout(&text, 20, theme.text(), &theme);
    assert!(layout.lines().iter().all(|line| line.width() <= 20));
    assert_eq!(
        layout.rejoined(),
        (1..=12)
            .map(|n| format!("{n}. abcde fghij klmno pqrst"))
            .collect::<Vec<_>>()
    );
    let rows: Vec<_> = layout.lines().iter().map(text_of).collect();
    assert!(rows.contains(&"10. abcde fghij".to_owned()));
    assert!(rows.contains(&"    klmno pqrst".to_owned()));
}
