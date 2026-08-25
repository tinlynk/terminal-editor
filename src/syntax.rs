use std::{
    ops::Range,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{LazyLock, Mutex},
};

use ratatui::style::Color;
use syntect::{
    easy::HighlightLines,
    highlighting::{
        Color as SyntectColor, Style, StyleModifier, Theme, ThemeItem, ThemeSettings,
    },
    parsing::{SyntaxReference, SyntaxSet},
    highlighting::ScopeSelectors,
};

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static SYNTAX_THEMES: LazyLock<Mutex<Vec<(SyntaxPalette, &'static Theme)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyntaxPalette {
    pub foreground: Color,
    pub comment: Color,
    pub string: Color,
    pub number: Color,
    pub keyword: Color,
    pub type_name: Color,
    pub function: Color,
    pub variable: Color,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxSpan {
    pub range: Range<usize>,
    pub color: Color,
}

enum SyntaxHighlighter {
    Standard {
        lines: HighlightLines<'static>,
    },
    Markdown {
        lines: HighlightLines<'static>,
        theme: &'static Theme,
        fence: Option<CodeFence>,
    },
}

struct CodeFence {
    delimiter: char,
    length: usize,
    lines: HighlightLines<'static>,
}

impl SyntaxHighlighter {
    #[cfg(test)]
    pub fn for_path(path: &Path) -> Option<Self> {
        Self::for_path_with_palette(path, default_palette())
    }

    pub fn for_path_with_palette(path: &Path, palette: SyntaxPalette) -> Option<Self> {
        let syntax = syntax_for_path(path)?;
        if syntax.name == "Plain Text" {
            return None;
        }
        let theme = syntax_theme(palette);
        let lines = HighlightLines::new(syntax, theme);
        Some(if syntax.name == "Markdown" {
            Self::Markdown {
                lines,
                theme,
                fence: None,
            }
        } else {
            Self::Standard { lines }
        })
    }

    pub fn highlight_line(&mut self, line: &str) -> Vec<SyntaxSpan> {
        match self {
            Self::Standard { lines } => highlight_spans(lines, line),
            Self::Markdown { lines, theme, fence } => {
                if let Some(active_fence) = fence {
                    if closes_fence(line, active_fence) {
                        *fence = None;
                        return highlight_spans(lines, line);
                    }
                    return highlight_spans(&mut active_fence.lines, line);
                }

                let spans = highlight_spans(lines, line);
                if let Some((delimiter, length, language)) = opens_fence(line)
                    && let Some(syntax) = syntax_for_fence(language)
                {
                    *fence = Some(CodeFence {
                        delimiter,
                        length,
                        lines: HighlightLines::new(syntax, theme),
                    });
                }
                spans
            }
        }
    }
}

fn highlight_spans(lines: &mut HighlightLines<'static>, line: &str) -> Vec<SyntaxSpan> {
    lines
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

fn opens_fence(line: &str) -> Option<(char, usize, &str)> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let delimiter = trimmed.chars().next()?;
    if delimiter != '`' && delimiter != '~' {
        return None;
    }
    let length = trimmed.chars().take_while(|character| *character == delimiter).count();
    if length < 3 {
        return None;
    }
    let language = trimmed[length..].split_whitespace().next()?;
    Some((delimiter, length, language))
}

fn closes_fence(line: &str, fence: &CodeFence) -> bool {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let length = trimmed
        .chars()
        .take_while(|character| *character == fence.delimiter)
        .count();
    length >= fence.length && trimmed[length..].trim().is_empty()
}

fn syntax_for_fence(language: &str) -> Option<&'static SyntaxReference> {
    let token = language.to_ascii_lowercase();
    let token = match token.as_str() {
        "shell" => "sh",
        value => value,
    };
    SYNTAX_SET.find_syntax_by_token(token).or_else(|| {
        ["bash", "zsh", "fish"].contains(&token).then(|| SYNTAX_SET.find_syntax_by_token("sh"))?
    })
}

fn syntax_for_path(path: &Path) -> Option<&'static syntect::parsing::SyntaxReference> {
    SYNTAX_SET
        .find_syntax_for_file(path)
        .ok()
        .flatten()
        .filter(|syntax| syntax.name != "Plain Text")
        .or_else(|| {
            let syntax_name = if path
                .file_name()?
                .to_str()?
                .eq_ignore_ascii_case("Vagrantfile")
            {
                "Ruby"
            } else {
                match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
                "md" | "markdown" | "mdown" | "mkdn" | "mdx" => "Markdown",
                "ts" | "tsx" | "mts" | "cts" => "JavaScript",
                _ => return None,
                }
            };
            SYNTAX_SET.find_syntax_by_name(syntax_name)
        })
}

#[derive(Default)]
pub struct SyntaxCache {
    path: Option<PathBuf>,
    revision: Option<u64>,
    palette: Option<SyntaxPalette>,
    highlighter: Option<SyntaxHighlighter>,
    spans: Vec<Vec<SyntaxSpan>>,
}

impl std::fmt::Debug for SyntaxCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SyntaxCache")
            .field("path", &self.path)
            .field("revision", &self.revision)
            .field("palette", &self.palette)
            .field("highlighted_lines", &self.spans.len())
            .finish()
    }
}

impl SyntaxCache {
    pub fn highlight_line(
        &mut self,
        path: Option<&Path>,
        revision: u64,
        palette: SyntaxPalette,
        line_index: usize,
        line: impl FnMut(usize) -> String,
    ) -> Vec<SyntaxSpan> {
        let path = path.map(Path::to_owned);
        if self.path != path || self.revision != Some(revision) || self.palette != Some(palette) {
            self.path = path.clone();
            self.revision = Some(revision);
            self.palette = Some(palette);
            self.highlighter = path
                .as_deref()
                .and_then(|path| SyntaxHighlighter::for_path_with_palette(path, palette));
            self.spans.clear();
        }
        let Some(highlighter) = self.highlighter.as_mut() else {
            return Vec::new();
        };
        let mut line = line;
        while self.spans.len() <= line_index {
            self.spans.push(highlighter.highlight_line(&line(self.spans.len())));
        }
        self.spans[line_index].clone()
    }
}

#[cfg(test)]
fn default_palette() -> SyntaxPalette {
    SyntaxPalette {
        foreground: Color::White,
        comment: Color::Rgb(0x6A, 0x99, 0x55),
        string: Color::Rgb(0xCE, 0x91, 0x78),
        number: Color::Rgb(0xB5, 0xCE, 0xA8),
        keyword: Color::Rgb(0x56, 0x9C, 0xD6),
        type_name: Color::Rgb(0x4E, 0xC9, 0xB0),
        function: Color::Rgb(0xDC, 0xDC, 0xAA),
        variable: Color::Rgb(0x9C, 0xDC, 0xFE),
    }
}

fn syntax_theme(palette: SyntaxPalette) -> &'static Theme {
    let mut themes = SYNTAX_THEMES.lock().expect("syntax theme cache lock is available");
    if let Some((_, theme)) = themes
        .iter()
        .find(|(cached_palette, _)| *cached_palette == palette)
    {
        return theme;
    }
    let theme = Box::leak(Box::new(build_syntax_theme(palette)));
    themes.push((palette, theme));
    theme
}

fn build_syntax_theme(palette: SyntaxPalette) -> Theme {
    Theme {
        name: None,
        author: None,
        settings: ThemeSettings {
            foreground: Some(syntect_color(palette.foreground)),
            ..ThemeSettings::default()
        },
        scopes: [
            ("comment.line, comment.block", palette.comment),
            ("string", palette.string),
            ("constant.numeric", palette.number),
            ("keyword", palette.keyword),
            ("storage.type", palette.type_name),
            ("entity.name.type", palette.type_name),
            ("entity.name.function", palette.function),
            ("variable", palette.variable),
            ("markup.heading", palette.keyword),
            ("markup.italic", palette.variable),
            ("markup.bold", palette.type_name),
            ("markup.underline.link", palette.function),
            ("markup.raw.inline", palette.string),
        ]
        .into_iter()
        .map(|(scope, color)| ThemeItem {
            scope: ScopeSelectors::from_str(scope).expect("static scope selector is valid"),
            style: StyleModifier {
                foreground: Some(syntect_color(color)),
                background: None,
                font_style: None,
            },
        })
        .collect(),
    }
}

fn syntect_color(color: Color) -> SyntectColor {
    match color {
        Color::Rgb(r, g, b) => SyntectColor { r, g, b, a: 0xFF },
        Color::Black => SyntectColor { r: 0, g: 0, b: 0, a: 0xFF },
        Color::Red => SyntectColor { r: 0xCD, g: 0x31, b: 0x31, a: 0xFF },
        Color::Green => SyntectColor { r: 0x0D, g: 0xBC, b: 0x79, a: 0xFF },
        Color::Yellow => SyntectColor { r: 0xE5, g: 0xE5, b: 0x10, a: 0xFF },
        Color::Blue => SyntectColor { r: 0x24, g: 0x78, b: 0xDB, a: 0xFF },
        Color::Magenta => SyntectColor { r: 0xBC, g: 0x3F, b: 0xBC, a: 0xFF },
        Color::Cyan => SyntectColor { r: 0x11, g: 0xA8, b: 0xCD, a: 0xFF },
        Color::Gray => SyntectColor { r: 0xCC, g: 0xCC, b: 0xCC, a: 0xFF },
        Color::DarkGray => SyntectColor { r: 0x76, g: 0x76, b: 0x76, a: 0xFF },
        Color::LightRed => SyntectColor { r: 0xF1, g: 0x4C, b: 0x4C, a: 0xFF },
        Color::LightGreen => SyntectColor { r: 0x23, g: 0xD1, b: 0x8B, a: 0xFF },
        Color::LightYellow => SyntectColor { r: 0xF5, g: 0xF5, b: 0x43, a: 0xFF },
        Color::LightBlue => SyntectColor { r: 0x3B, g: 0x8E, b: 0xEA, a: 0xFF },
        Color::LightMagenta => SyntectColor { r: 0xD6, g: 0x70, b: 0xD6, a: 0xFF },
        Color::LightCyan => SyntectColor { r: 0x29, g: 0xB8, b: 0xDB, a: 0xFF },
        Color::White => SyntectColor { r: 0xFF, g: 0xFF, b: 0xFF, a: 0xFF },
        Color::Reset | Color::Indexed(_) => SyntectColor { r: 0xFF, g: 0xFF, b: 0xFF, a: 0xFF },
    }
}

fn color(style: Style) -> Color {
    Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, path::Path};

    use super::*;

    #[test]
    fn rust_paths_produce_syntax_spans() {
        let mut highlighter = SyntaxHighlighter::for_path(Path::new("main.rs")).unwrap();

        assert!(!highlighter.highlight_line("fn main() {}").is_empty());
    }

    #[test]
    fn markdown_and_typescript_paths_produce_syntax_spans() {
        for path in ["README.md", "guide.mdx"] {
            let mut markdown = SyntaxHighlighter::for_path(Path::new(path)).unwrap();
            assert!(markdown
                .highlight_line("# Heading with *emphasis* and `code`")
                .iter()
                .any(|span| span.color != default_palette().foreground));
        }
        for path in ["app.ts", "view.tsx", "module.mts", "legacy.cts"] {
            let mut typescript = SyntaxHighlighter::for_path(Path::new(path)).unwrap();
            assert!(!typescript.highlight_line("const value: number = 1;").is_empty());
        }
    }

    #[test]
    fn vagrantfile_paths_use_ruby_highlighting() {
        let mut highlighter = SyntaxHighlighter::for_path(Path::new("Vagrantfile")).unwrap();

        assert!(highlighter
            .highlight_line("Vagrant.configure(\"2\") do |config|")
            .iter()
            .any(|span| span.color != default_palette().foreground));
    }

    #[test]
    fn markdown_fenced_blocks_use_their_language_syntax() {
        for (language, code) in [
            ("rust", "let value = \"text\";"),
            ("toml", "value = \"text\""),
            ("java", "String value = \"text\";"),
            ("sh", "echo \"$HOME\""),
            ("bash", "echo \"$HOME\""),
            ("zsh", "echo \"$HOME\""),
            ("fish", "echo \"$HOME\""),
        ] {
            let mut highlighter = SyntaxHighlighter::for_path(Path::new("README.md")).unwrap();

            highlighter.highlight_line(&format!("```{language}"));
            assert!(highlighter
                .highlight_line(code)
                .iter()
                .any(|span| span.color != default_palette().foreground));
            highlighter.highlight_line("```");
            assert!(highlighter
                .highlight_line("# Markdown heading")
                .iter()
                .any(|span| span.color != default_palette().foreground));
        }
    }

    #[test]
    fn unknown_markdown_fence_tags_restore_markdown_after_closing() {
        let mut highlighter = SyntaxHighlighter::for_path(Path::new("README.md")).unwrap();

        highlighter.highlight_line("```unknown-language");
        highlighter.highlight_line("unparsed content");
        highlighter.highlight_line("```");

        assert!(highlighter
            .highlight_line("# Markdown heading")
            .iter()
            .any(|span| span.color != default_palette().foreground));
    }

    #[test]
    fn plain_text_paths_are_not_highlighted() {
        assert!(SyntaxHighlighter::for_path(Path::new("notes.txt")).is_none());
        assert!(SyntaxHighlighter::for_path(Path::new("Untitled")).is_none());
    }

    #[test]
    fn theme_palette_controls_comment_color() {
        let palette = SyntaxPalette {
            comment: Color::Red,
            ..default_palette()
        };
        let mut highlighter =
            SyntaxHighlighter::for_path_with_palette(Path::new("main.rs"), palette).unwrap();

        let spans = highlighter.highlight_line("// comment");

        assert!(
            spans
                .iter()
                .any(|span| span.color == Color::Rgb(0xCD, 0x31, 0x31))
        );
    }

    #[test]
    fn cache_reuses_lines_until_the_document_revision_changes() {
        let mut cache = SyntaxCache::default();
        let calls = Cell::new(0);
        let line = |index| {
            calls.set(calls.get() + 1);
            format!("// {index}")
        };

        cache.highlight_line(
            Some(Path::new("main.rs")),
            1,
            default_palette(),
            2,
            line,
        );
        assert_eq!(calls.get(), 3);
        cache.highlight_line(
            Some(Path::new("main.rs")),
            1,
            default_palette(),
            2,
            |_| panic!("cached line should not be reparsed"),
        );
        cache.highlight_line(
            Some(Path::new("main.rs")),
            2,
            default_palette(),
            2,
            line,
        );
        assert_eq!(calls.get(), 6);
    }

    #[test]
    fn cache_revision_change_resets_markdown_fence_state() {
        let mut cache = SyntaxCache::default();

        cache.highlight_line(
            Some(Path::new("README.md")),
            1,
            default_palette(),
            1,
            |index| ["```rust", "let value = \"text\";"][index].to_owned(),
        );
        let spans = cache.highlight_line(
            Some(Path::new("README.md")),
            2,
            default_palette(),
            0,
            |_| "# Markdown heading".to_owned(),
        );

        assert!(spans
            .iter()
            .any(|span| span.color != default_palette().foreground));
    }
}