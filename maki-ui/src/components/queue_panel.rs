use std::borrow::Cow;

use crate::components::keybindings::key;
use crate::theme;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

const ELLIPSIS: &str = "...";
const QUEUE_LABEL: &str = " Queue ";
const FOCUSED_HINT: &str = " - Enter to delete";

pub struct QueueEntry<'a> {
    pub text: Cow<'a, str>,
    pub color: ratatui::style::Color,
}

pub fn height(queue_len: usize) -> u16 {
    if queue_len == 0 {
        0
    } else {
        queue_len as u16 + 2
    }
}

pub fn view(frame: &mut Frame, area: Rect, entries: &[QueueEntry], focus: Option<usize>) {
    if entries.is_empty() {
        return;
    }
    let content_width = area.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let flat = entry.text.replace('\n', " ");
            let (style, hint_parts) = if focus == Some(i) {
                (theme::current().queue_delete, ("", FOCUSED_HINT, ""))
            } else if i == 0 {
                (
                    Style::new().fg(entry.color),
                    (" - ", key::POP_QUEUE.label, " to delete"),
                )
            } else {
                (Style::new().fg(entry.color), ("", "", ""))
            };
            truncate_line(&flat, content_width, style, hint_parts)
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focus.is_some() {
            theme::current().queue_delete
        } else {
            theme::current().panel_border
        })
        .title_top(Line::from(QUEUE_LABEL).left_aligned())
        .title_style(theme::current().panel_title);

    let paragraph = Paragraph::new(lines)
        .style(Style::new().fg(theme::current().foreground))
        .block(block);

    frame.render_widget(paragraph, area);
}

fn truncate_line(
    text: &str,
    max_width: usize,
    style: Style,
    hint: (&'static str, &'static str, &'static str),
) -> Line<'static> {
    use crate::truncate::head_fitting;
    use unicode_width::UnicodeWidthStr;

    let hint_style = theme::current().tool_dim;
    let hint_width: usize = [hint.0, hint.1, hint.2].iter().map(|s| s.width()).sum();
    let available = max_width.saturating_sub(hint_width);

    let (head, fits) = head_fitting(text, available);
    let (text_span, ellipsis) = if fits {
        (Span::styled(head.to_string(), style), None)
    } else {
        // Reserve room for the ellipsis, then keep as much of the head as fits.
        let (head, _) = head_fitting(text, available.saturating_sub(ELLIPSIS.width()));
        (
            Span::styled(head.to_string(), style),
            Some(Span::styled(ELLIPSIS, hint_style)),
        )
    };

    let mut spans = vec![text_span];
    spans.extend(ellipsis);
    if hint_width > 0 {
        spans.push(Span::styled(hint.0, hint_style));
        spans.push(Span::styled(hint.1, hint_style));
        spans.push(Span::styled(hint.2, hint_style));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    use test_case::test_case;

    #[test]
    fn height_includes_borders() {
        assert_eq!(height(0), 0);
        assert_eq!(height(1), 3);
        assert_eq!(height(3), 5);
    }

    const HINT: (&str, &str, &str) = (" - hint", "", "");
    const NO_HINT: (&str, &str, &str) = ("", "", "");
    fn style() -> Style {
        Style::new().fg(theme::current().foreground)
    }
    fn span_texts<'a>(line: &'a Line<'a>) -> Vec<&'a str> {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .filter(|s| !s.is_empty())
            .collect()
    }

    const HINT_STR: &str = " - hint";
    #[test_case("hello", 10, NO_HINT, &["hello"]                                          ; "no_hint_short")]
    #[test_case("abcdefghij", 7, NO_HINT, &["abcd", ELLIPSIS]                             ; "no_hint_truncated")]
    #[test_case("abcde", 5, NO_HINT, &["abcde"]                                           ; "no_hint_exact_width")]
    #[test_case("abcdef", 2, NO_HINT, &[ELLIPSIS]                                     ; "no_hint_tiny_width")]
    #[test_case("●abc", 5, NO_HINT, &["●abc"]                                         ; "no_hint_multibyte_fits_cells")]
    #[test_case("●●●", 8, NO_HINT, &["●●●"]                                              ; "no_hint_wide_fits_cells")]
    #[test_case("●●●●", 3, NO_HINT, &[ELLIPSIS]                                       ; "no_hint_wide_ellipsis_only")]
    #[test_case("日本語", 5, NO_HINT, &["日", ELLIPSIS]                                   ; "no_hint_cjk_truncates_on_grapheme")]
    #[test_case("ab👩👩", 5, NO_HINT, &["ab", ELLIPSIS]                                  ; "no_hint_emoji_is_wide")]
    #[test_case("👨‍👩", 1, NO_HINT, &[ELLIPSIS]                                          ; "no_hint_zwj_cluster_never_split")]
    #[test_case("hello", 20, HINT, &["hello", HINT_STR]                                   ; "hint_short")]
    #[test_case("abcdefghijklmnopqrstuvwxyz", 18, HINT, &["abcdefgh", ELLIPSIS, HINT_STR]  ; "hint_truncated")]
    #[test_case("ab", 9, HINT, &["ab", HINT_STR]                                          ; "hint_exact_fit")]
    fn truncate_line_cases(
        input: &str,
        width: usize,
        hint: (&'static str, &'static str, &'static str),
        expected: &[&str],
    ) {
        assert_eq!(
            span_texts(&truncate_line(input, width, style(), hint)),
            expected
        );
    }

    /// Every cut on random Unicode stays within the panel width and never
    /// emits a replacement char, for any width a real panel can have.
    #[test]
    fn randomized_lines_stay_within_budget() {
        let mut rng = fastrand::Rng::with_seed(0x0be7);
        let pools: &[&[char]] = &[
            &['a', 'b', ' ', '-', '_'],
            &['\u{65e5}', '\u{672c}', '\u{8a9e}'],
            &['\u{1f600}', '\u{1f5a5}'],
            &['\u{200d}', '\u{fe0f}', '\u{0301}'],
        ];
        for _ in 0..300 {
            let len = rng.usize(0..24);
            let text: String = (0..len)
                .map(|_| {
                    let pool = pools[rng.usize(0..pools.len())];
                    pool[rng.usize(0..pool.len())]
                })
                .collect();
            // Below the ellipsis width the marker alone cannot fit, which is
            // the pre-existing tiny-panel behaviour; above it the line must.
            let min = unicode_width::UnicodeWidthStr::width(ELLIPSIS);
            for width in min..=20 {
                let line = truncate_line(&text, width, style(), NO_HINT);
                assert!(
                    line.width() <= width,
                    "{text:?} at {width} is {} wide",
                    line.width()
                );
                let joined: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                assert!(!joined.contains('\u{fffd}'));
            }
        }
    }
}
