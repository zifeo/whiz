use ansi_to_tui::IntoText;
use anyhow::anyhow;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, StyledGrapheme};
use regex::Regex;

#[derive(Clone, Debug)]
pub struct ColorOption {
    pub regex: Regex,
    pub color: Color,
}

impl ColorOption {
    pub fn new(regex: Regex, color: Color) -> Self {
        Self { regex, color }
    }

    pub fn from(color_config: (&String, &String)) -> anyhow::Result<Self> {
        let (regex, color_str) = color_config;
        let regex = Regex::new(regex)?;
        let color = ColorOption::parse_color(color_str)?;
        Ok(Self { regex, color })
    }

    pub fn parse_color(str: &str) -> anyhow::Result<Color> {
        if str.starts_with('#') {
            let rgb = u32::from_str_radix(str.trim_start_matches('#'), 16)?;
            let r = ((rgb & 0x00FF0000) >> 16) as u8;
            let g = ((rgb & 0x0000FF00) >> 8) as u8;
            let b = (rgb & 0x000000FF) as u8;
            return Ok(Color::Rgb(r, g, b));
        }

        match str.to_ascii_lowercase().as_str() {
            "red" => Ok(Color::Red),
            "blue" => Ok(Color::Blue),
            "gray" => Ok(Color::Gray),
            "cyan" => Ok(Color::Cyan),
            "black" => Ok(Color::Black),
            "green" => Ok(Color::Green),
            "white" => Ok(Color::White),
            "yellow" => Ok(Color::Yellow),
            "magenta" => Ok(Color::Magenta),
            "darkgray" => Ok(Color::DarkGray),
            "lightred" => Ok(Color::LightRed),
            "lightblue" => Ok(Color::LightBlue),
            "lightcyan" => Ok(Color::LightCyan),
            "lightgreen" => Ok(Color::LightGreen),
            "lightyellow" => Ok(Color::LightYellow),
            "lightmagenta" => Ok(Color::LightMagenta),
            other => Err(anyhow!("unsupported color: {:?}", other)),
        }
    }
}

impl PartialEq for ColorOption {
    fn eq(&self, other: &Self) -> bool {
        self.regex.as_str() == other.regex.as_str() && self.color == other.color
    }
}

pub struct Colorizer<'b> {
    colors: &'b Vec<ColorOption>,
    base_style: Style,
}

impl<'b> Colorizer<'b> {
    pub fn new(colors: &'b Vec<ColorOption>, base_style: Style) -> Self {
        Self { colors, base_style }
    }

    ///
    /// Patches style of input [`&str`] according to stored [`ColorOption`]'s.
    /// Each color is applied sequentially.
    ///
    /// Returns vector of patched lines.
    ///
    pub fn patch_text<'a>(&self, str: &'a str) -> Vec<Line<'a>> {
        let mut text = str.into_text().unwrap();

        text.patch_style(self.base_style);

        if self.colors.is_empty() {
            // We don't have color options.
            // Just return base-styled lines.
            return text.lines;
        }

        // Iterate over lines and patch them one-by-one
        text.lines
            .iter()
            .map(|line| {
                let mut styled_line = line.clone();
                let pure_str = Colorizer::line_as_string(line);
                for opt in self.colors {
                    styled_line =
                        self.merge_lines(&styled_line, &self.apply_color_option(&pure_str, opt));
                }
                styled_line
            })
            .collect()
    }

    fn line_as_string(line: &Line) -> String {
        line.spans
            .iter()
            .map(|s| s.content.to_string())
            .collect::<Vec<_>>()
            .join("")
    }

    fn uncolored<'a>(&self, content: &'a str) -> Span<'a> {
        Span::styled(content, self.base_style)
    }

    fn colored<'a>(&self, content: &'a str, color: Color) -> Span<'a> {
        Span::styled(content, self.base_style.fg(color))
    }

    ///
    /// Creates a new [`Line<'c>`] from the given input [`Line<'a>`]'s.
    ///
    /// Byte contents of the text should be equal. Only grapheme styles
    /// can differ. RHS styles always has priority in contrast with LHS.
    ///
    fn merge_lines<'a, 'c>(&self, lhs: &Line<'a>, rhs: &Line<'a>) -> Line<'c> {
        let lhs_graphemes = lhs.styled_graphemes(self.base_style);
        let rhs_graphemes = rhs.styled_graphemes(self.base_style);

        let merged_graphemes: Vec<StyledGrapheme<'_>> = lhs_graphemes
            .zip(rhs_graphemes)
            .map(|(l, r)| {
                assert_eq!(l.symbol, r.symbol, "Symbols should be always equal here");
                if r.style.fg.is_none() {
                    l
                } else {
                    r
                }
            })
            .collect();

        let mut spans = Vec::new();
        let mut outer = merged_graphemes.iter();
        while let Some(grapheme) = outer.next() {
            let mut content = String::from(grapheme.symbol);
            let mut inner = outer.clone();

            while let Some(StyledGrapheme { symbol, style }) = inner.next() {
                if *style == grapheme.style {
                    content += symbol;
                    outer = inner.clone();
                } else {
                    break;
                }
            }

            spans.push(Span::styled(content, grapheme.style));
        }

        Line::from(spans)
    }

    ///
    /// Splits pure [`&str`] into vector of [`Span`]'s by applying regex pattern stored
    /// in [`ColorOption`].
    ///
    /// All matched substrings are colorized to corresponding color.
    /// Any other unmatched substrings have "base" style.
    ///
    fn apply_color_option<'a>(&self, s: &'a str, opt: &ColorOption) -> Line<'a> {
        let mut last = 0;
        let mut result = Vec::new();

        for m in opt.regex.find_iter(s) {
            if last != m.start() {
                let unmatched = self.uncolored(&s[last..m.start()]);
                result.push(unmatched);
            }
            let matched = self.colored(&s[m.start()..m.end()], opt.color);
            result.push(matched);
            last = m.end();
        }

        if last < s.len() {
            result.push(self.uncolored(&s[last..]));
        }

        Line::from(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn merge_colored_lines() {
        let lhs = Line::from(vec![
            Span::styled("S", Style::default().fg(Color::Magenta)),
            Span::styled("hould be ", Style::default()),
            Span::styled("SPLITTED", Style::default().fg(Color::Magenta)),
            Span::styled(" into ", Style::default()),
            Span::styled("COLORED", Style::default().fg(Color::Magenta)),
            Span::styled(" spans", Style::default()),
        ]);

        let rhs = Line::from(vec![
            Span::styled("Should be ", Style::default().fg(Color::Red)),
            Span::styled("SPLIT", Style::default().fg(Color::Yellow)),
            Span::styled("TED", Style::default()),
            Span::styled(" into ", Style::default().fg(Color::Cyan)),
            Span::styled("COLORED s", Style::default().fg(Color::Green)),
            Span::styled("pans", Style::default().fg(Color::DarkGray)),
        ]);

        let colored_opt = Vec::new();
        let colorizer = Colorizer::new(&colored_opt, Style::default());

        assert_eq!(
            colorizer.merge_lines(&lhs, &rhs).spans,
            vec![
                Span::styled("Should be ", Style::default().fg(Color::Red)),
                Span::styled("SPLIT", Style::default().fg(Color::Yellow)),
                Span::styled("TED", Style::default().fg(Color::Magenta)),
                Span::styled(" into ", Style::default().fg(Color::Cyan)),
                Span::styled("COLORED s", Style::default().fg(Color::Green)),
                Span::styled("pans", Style::default().fg(Color::DarkGray)),
            ]
        );

        assert_eq!(
            colorizer.merge_lines(&rhs, &lhs).spans,
            vec![
                Span::styled("S", Style::default().fg(Color::Magenta)),
                Span::styled("hould be ", Style::default().fg(Color::Red)),
                Span::styled("SPLITTED", Style::default().fg(Color::Magenta)),
                Span::styled(" into ", Style::default().fg(Color::Cyan)),
                Span::styled("COLORED", Style::default().fg(Color::Magenta)),
                Span::styled(" s", Style::default().fg(Color::Green)),
                Span::styled("pans", Style::default().fg(Color::DarkGray)),
            ]
        );
    }

    #[test]
    fn split_string_into_colored_parts() {
        let test_string = "Should be SPLITTED into COLORED spans";
        let colored_opt = Vec::new();
        let colorizer = Colorizer::new(&colored_opt, Style::default());

        let actual_spans = colorizer.apply_color_option(
            test_string,
            &ColorOption::new(
                Regex::from_str("[A-Z]+").unwrap(),
                ColorOption::parse_color("magenta").unwrap(),
            ),
        );

        let expected_spans = Line::from(vec![
            Span::styled("S", Style::default().fg(Color::Magenta)),
            Span::styled("hould be ", Style::default()),
            Span::styled("SPLITTED", Style::default().fg(Color::Magenta)),
            Span::styled(" into ", Style::default()),
            Span::styled("COLORED", Style::default().fg(Color::Magenta)),
            Span::styled(" spans", Style::default()),
        ]);

        assert_eq!(actual_spans, expected_spans);
    }

    #[test]
    fn patch_ansi() {
        let ansi_string = "\u{1b}[31mHelloWorld\u{1b}[0m"; // red-line colored ANSI string
        let base_style = Style::default();
        let color_opts = vec![
            ColorOption::new(
                Regex::from_str("He").unwrap(),
                ColorOption::parse_color("yellow").unwrap(),
            ),
            ColorOption::new(
                Regex::from_str("Wor").unwrap(),
                ColorOption::parse_color("green").unwrap(),
            ),
        ];

        let colorizer = Colorizer::new(&color_opts, base_style);
        let patched = colorizer.patch_text(ansi_string);
        let expected = vec![
            Span::styled("He", base_style.fg(Color::Yellow)),
            Span::styled("llo", base_style.fg(Color::Red)),
            Span::styled("Wor", base_style.fg(Color::Green)),
            Span::styled("ld", base_style.fg(Color::Red)),
        ];

        assert_eq!(patched.len(), 1);
        assert_eq!(expected, patched.first().unwrap().spans);
    }

    #[test]
    fn patch_line() {
        let test_string = "The variablE#nAmEs####next. http://localhost:8080";
        let color_opts = vec![
            ColorOption::new(
                Regex::from_str("#+").unwrap(),
                ColorOption::parse_color("#eee").unwrap(),
            ),
            ColorOption::new(
                Regex::from_str("[a-z]\\#+[a-z]").unwrap(),
                ColorOption::parse_color("blue").unwrap(),
            ),
            ColorOption::new(
                Regex::from_str("[A-Z]").unwrap(),
                ColorOption::parse_color("green").unwrap(),
            ),
            ColorOption::new(
                Regex::from_str("^The").unwrap(),
                ColorOption::parse_color("yellow").unwrap(),
            ),
            ColorOption::new(
                Regex::from_str("http://(.*)").unwrap(),
                ColorOption::parse_color("#def").unwrap(),
            ),
        ];

        let base_style = Style::default();
        let colorizer = Colorizer::new(&color_opts, base_style);
        let patched = colorizer.patch_text(test_string);

        let expected = vec![
            Span::styled("The", base_style.fg(Color::Yellow)),
            Span::styled(" variabl", base_style),
            Span::styled("E", base_style.fg(Color::Green)),
            Span::styled("#", base_style.fg(Color::Rgb(0, 14, 238))),
            Span::styled("n", base_style),
            Span::styled("A", base_style.fg(Color::Green)),
            Span::styled("m", base_style),
            Span::styled("E", base_style.fg(Color::Green)),
            Span::styled("s####n", base_style.fg(Color::Blue)),
            Span::styled("ext. ", base_style),
            Span::styled(
                "http://localhost:8080",
                base_style.fg(Color::Rgb(0, 13, 239)),
            ),
        ];

        assert_eq!(patched.len(), 1);
        assert_eq!(expected, patched.first().unwrap().spans);
    }
}
