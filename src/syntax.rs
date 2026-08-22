use std::{ops::Range, path::Path, sync::LazyLock};

use ratatui::style::Color;
use syntect::{
    easy::HighlightLines,
    highlighting::{Style, Theme, ThemeSet},
    parsing::SyntaxSet,
};

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME: LazyLock<Theme> = LazyLock::new(|| {
    ThemeSet::load_defaults()
        .themes
        .get("base16-ocean.dark")
        .expect("bundled Syntect theme exists")
        .clone()
});

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxSpan {
    pub range: Range<usize>,
    pub color: Color,
}

pub struct SyntaxHighlighter {
    lines: HighlightLines<'static>,
}

impl SyntaxHighlighter {
    pub fn for_path(path: &Path) -> Option<Self> {
        let syntax = SYNTAX_SET.find_syntax_for_file(path).ok()??;
        if syntax.name == "Plain Text" {
            return None;
        }
        Some(Self {
            lines: HighlightLines::new(syntax, &THEME),
        })
    }

    pub fn highlight_line(&mut self, line: &str) -> Vec<SyntaxSpan> {
        self.lines
            .highlight_line(line, &SYNTAX_SET)
            .unwrap_or_default()
            .into_iter()
            .map(|(style, text)| {
                let start = text.as_ptr() as usize - line.as_ptr() as usize;
                SyntaxSpan {
                    range: start..start + text.len(),
                    color: color(style),
                }
            })
            .collect()
    }
}

fn color(style: Style) -> Color {
    Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn rust_paths_produce_syntax_spans() {
        let mut highlighter = SyntaxHighlighter::for_path(Path::new("main.rs")).unwrap();

        assert!(!highlighter.highlight_line("fn main() {}").is_empty());
    }

    #[test]
    fn plain_text_paths_are_not_highlighted() {
        assert!(SyntaxHighlighter::for_path(Path::new("notes.txt")).is_none());
        assert!(SyntaxHighlighter::for_path(Path::new("Untitled")).is_none());
    }
}