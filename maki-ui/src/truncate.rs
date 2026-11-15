//! Display-width-aware truncation shared by UI text paths.
//!
//! Terminal layouts measure in cells, not bytes or chars: a CJK glyph or an
//! emoji occupies two cells, while a byte may be a fraction of a glyph. These
//! helpers cut on grapheme clusters and count display cells so callers never
//! overflow a rect or split a multi-codepoint grapheme (ZWJ sequences,
//! variation selectors, combining marks).

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Returns the longest prefix of `text` that fits in `max_width` display cells.
///
/// The cut lands on a grapheme boundary. The boolean is `true` when all of
/// `text` fit, so callers can skip appending an ellipsis.
pub(crate) fn head_fitting(text: &str, max_width: usize) -> (&str, bool) {
    if text.width() <= max_width {
        return (text, true);
    }
    let mut width = 0;
    let mut end = 0;
    for grapheme in text.graphemes(true) {
        let g_width = grapheme.width();
        if width + g_width > max_width {
            break;
        }
        width += g_width;
        end += grapheme.len();
    }
    (&text[..end], false)
}

/// Returns the longest suffix of `text` that fits in `max_width` display cells.
///
/// The cut lands on a grapheme boundary. The boolean is `true` when all of
/// `text` fit, so callers can skip prepending an ellipsis.
pub(crate) fn tail_fitting(text: &str, max_width: usize) -> (&str, bool) {
    if text.width() <= max_width {
        return (text, true);
    }
    let mut width = 0;
    let mut start = text.len();
    for grapheme in text.graphemes(true).rev() {
        let g_width = grapheme.width();
        if width + g_width > max_width {
            break;
        }
        width += g_width;
        start -= grapheme.len();
    }
    (&text[start..], false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(s: &str) -> usize {
        UnicodeWidthStr::width(s)
    }

    #[test]
    fn head_fits_entire_text_unchanged() {
        assert_eq!(head_fitting("hello", 10), ("hello", true));
        assert_eq!(head_fitting("hello", 5), ("hello", true));
    }

    #[test]
    fn head_cuts_on_grapheme_boundaries() {
        let (head, fits) = head_fitting("日本語", 4);
        assert!(!fits);
        assert_eq!(head, "日本");
        assert!(width(head) <= 4);
    }

    #[test]
    fn tail_fits_entire_text_unchanged() {
        assert_eq!(tail_fitting("hello", 10), ("hello", true));
        assert_eq!(tail_fitting("hello", 5), ("hello", true));
    }

    #[test]
    fn tail_cuts_on_grapheme_boundaries() {
        let (tail, fits) = tail_fitting("日本語", 4);
        assert!(!fits);
        assert_eq!(tail, "本語");
        assert!(width(tail) <= 4);
    }

    #[test]
    fn zwj_cluster_is_never_split() {
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        let (head, fits) = head_fitting(family, 1);
        if fits {
            assert_eq!(head, family);
        } else {
            // Too wide for the budget, so nothing is kept, but crucially no
            // partial cluster is emitted.
            assert!(head.is_empty());
        }
        assert!(width(head) <= 1);
    }

    #[test]
    fn variation_selector_is_never_orphaned() {
        let warn = "warn \u{26a0}\u{fe0f}";
        for max in 0..=width(warn) + 2 {
            let (head, _) = head_fitting(warn, max);
            assert!(warn.starts_with(head));
            assert!(!head.ends_with('\u{fe0f}') || head.ends_with("\u{26a0}\u{fe0f}"));
        }
    }

    /// Deterministic property check over a broad spread of Unicode scalar
    /// values: every cut stays within budget and remains a valid prefix/suffix.
    #[test]
    fn randomized_unicode_strings_truncate_safely() {
        let mut rng = fastrand::Rng::with_seed(0x5eed);
        let pools: &[&[char]] = &[
            &['a', 'b', 'c', ' ', '-', '_'],
            &['\u{03b1}', '\u{03b2}', '\u{03c4}', '\u{03b5}'],
            &['\u{65e5}', '\u{672c}', '\u{8a9e}'],
            &['\u{1f600}', '\u{1f389}', '\u{1f5a5}'],
            &['\u{200d}', '\u{fe0f}', '\u{0301}'],
        ];
        for _ in 0..500 {
            let len = rng.usize(0..24);
            let text: String = (0..len)
                .map(|_| {
                    let pool = pools[rng.usize(0..pools.len())];
                    pool[rng.usize(0..pool.len())]
                })
                .collect();
            for max in 0..=20 {
                let (head, head_fits) = head_fitting(&text, max);
                assert!(width(head) <= max, "head {head:?} over {max}");
                assert!(text.starts_with(head), "head is not a prefix");
                assert_eq!(head_fits, width(&text) <= max);

                let (tail, tail_fits) = tail_fitting(&text, max);
                assert!(width(tail) <= max, "tail {tail:?} over {max}");
                assert!(text.ends_with(tail), "tail is not a suffix");
                assert_eq!(tail_fits, width(&text) <= max);
            }
        }
    }
}
