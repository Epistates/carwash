//! Color themes and glyph sets.

use ratatui::style::{Color, Modifier, Style};

/// Semantic colors. Backgrounds stay the terminal's own, so themes blend with any setup.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub text: Color,
    pub subtle: Color,
    pub muted: Color,
    /// Highlighted row background.
    pub surface: Color,
    pub border: Color,
    pub accent: Color,
    pub accent2: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
    pub truecolor: bool,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    pub const NAMES: [&'static str; 5] = ["gestalt", "latte", "nord", "dracula", "ansi"];

    /// Catppuccin Mocha.
    pub const GESTALT: Self = Self {
        name: "gestalt",
        text: rgb(0xcdd6f4),
        subtle: rgb(0xa6adc8),
        muted: rgb(0x6c7086),
        surface: rgb(0x313244),
        border: rgb(0x45475a),
        accent: rgb(0xb4befe),
        accent2: rgb(0xcba6f7),
        success: rgb(0xa6e3a1),
        warning: rgb(0xf9e2af),
        error: rgb(0xf38ba8),
        info: rgb(0x89b4fa),
        truecolor: true,
    };

    /// Catppuccin Latte, for light terminals.
    pub const LATTE: Self = Self {
        name: "latte",
        text: rgb(0x4c4f69),
        subtle: rgb(0x6c6f85),
        muted: rgb(0x9ca0b0),
        surface: rgb(0xccd0da),
        border: rgb(0xbcc0cc),
        accent: rgb(0x7287fd),
        accent2: rgb(0x8839ef),
        success: rgb(0x40a02b),
        warning: rgb(0xdf8e1d),
        error: rgb(0xd20f39),
        info: rgb(0x1e66f5),
        truecolor: true,
    };

    pub const NORD: Self = Self {
        name: "nord",
        text: rgb(0xeceff4),
        subtle: rgb(0xd8dee9),
        muted: rgb(0x616e88),
        surface: rgb(0x3b4252),
        border: rgb(0x434c5e),
        accent: rgb(0x88c0d0),
        accent2: rgb(0x81a1c1),
        success: rgb(0xa3be8c),
        warning: rgb(0xebcb8b),
        error: rgb(0xbf616a),
        info: rgb(0x5e81ac),
        truecolor: true,
    };

    pub const DRACULA: Self = Self {
        name: "dracula",
        text: rgb(0xf8f8f2),
        subtle: rgb(0xd6d6d0),
        muted: rgb(0x6272a4),
        surface: rgb(0x44475a),
        border: rgb(0x44475a),
        accent: rgb(0xbd93f9),
        accent2: rgb(0xff79c6),
        success: rgb(0x50fa7b),
        warning: rgb(0xf1fa8c),
        error: rgb(0xff5555),
        info: rgb(0x8be9fd),
        truecolor: true,
    };

    /// The terminal's own 16-color palette.
    pub const ANSI: Self = Self {
        name: "ansi",
        text: Color::Reset,
        subtle: Color::Gray,
        muted: Color::DarkGray,
        surface: Color::DarkGray,
        border: Color::DarkGray,
        accent: Color::Cyan,
        accent2: Color::Magenta,
        success: Color::Green,
        warning: Color::Yellow,
        error: Color::Red,
        info: Color::Blue,
        truecolor: false,
    };

    /// Theme by name; truecolor themes fall back to `ansi` when the terminal lacks 24-bit color.
    pub fn named(name: &str) -> Self {
        let theme = match name {
            "latte" => Self::LATTE,
            "nord" => Self::NORD,
            "dracula" => Self::DRACULA,
            "ansi" => Self::ANSI,
            _ => Self::GESTALT,
        };
        if theme.truecolor && !supports_truecolor() {
            Self::ANSI
        } else {
            theme
        }
    }

    pub fn next(self) -> Self {
        let i = Self::NAMES
            .iter()
            .position(|n| *n == self.name)
            .unwrap_or(0);
        Self::named(Self::NAMES[(i + 1) % Self::NAMES.len()])
    }

    pub fn fg(&self, color: Color) -> Style {
        Style::new().fg(color)
    }

    pub fn text(&self) -> Style {
        self.fg(self.text)
    }

    pub fn muted(&self) -> Style {
        self.fg(self.muted)
    }

    pub fn subtle(&self) -> Style {
        self.fg(self.subtle)
    }

    pub fn bold(&self, color: Color) -> Style {
        self.fg(color).add_modifier(Modifier::BOLD)
    }

    pub fn selected_row(&self) -> Style {
        Style::new().bg(self.surface).add_modifier(Modifier::BOLD)
    }

    /// Color for a byte count: large amounts stand out.
    pub fn size(&self, bytes: u64) -> Style {
        const GB: u64 = 1_000_000_000;
        match bytes {
            b if b >= 10 * GB => self.bold(self.error),
            b if b >= GB => self.bold(self.warning),
            b if b >= 100_000_000 => self.text(),
            _ => self.subtle(),
        }
    }

    /// Brand color for an ecosystem badge (GitHub Linguist colors where they exist).
    pub fn ecosystem(&self, key: &str) -> Color {
        if !self.truecolor {
            return match key {
                "rust" | "swift" | "java" | "maven" | "gradle" => Color::Red,
                "node" | "deno" | "python" | "php" => Color::Yellow,
                "go" | "dart" | "zig" => Color::Cyan,
                "dotnet" | "elixir" | "haskell" => Color::Magenta,
                _ => Color::Gray,
            };
        }
        rgb(match key {
            "rust" => 0xdea584,
            "node" => 0xf1e05a,
            "deno" => 0x70ffaf,
            "python" => 0x3572a5,
            "go" => 0x00add8,
            "maven" | "gradle" => 0xb07219,
            "scala" => 0xc22d40,
            "dotnet" => 0x178600,
            "swift" | "xcode" | "cocoapods" | "carthage" => 0xf05138,
            "dart" => 0x00b4ab,
            "elixir" => 0x6e4a7e,
            "erlang" => 0xb83998,
            "gleam" => 0xffaff3,
            "haskell" => 0x5e5086,
            "ocaml" => 0xef7a08,
            "zig" => 0xec915c,
            "cmake" | "meson" | "xmake" => 0x6866fb,
            "php" => 0x4f5d95,
            "ruby" => 0x701516,
            "terraform" => 0x7b42bc,
            "unity" | "unreal" | "godot" => 0x478cbf,
            "r" => 0x198ce7,
            "julia" => 0xa270ba,
            "nix" => 0x7e7eff,
            _ => 0x9399b2,
        })
    }
}

fn supports_truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit")
        || std::env::var("TERM_PROGRAM").is_ok_and(|v| {
            matches!(
                v.as_str(),
                "iTerm.app" | "WezTerm" | "ghostty" | "vscode" | "WarpTerminal"
            )
        })
}

/// Glyph set for marks, tree expanders and bars.
#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    pub marked: &'static str,
    pub unmarked: &'static str,
    pub partial: &'static str,
    pub locked: &'static str,
    pub expanded: &'static str,
    pub collapsed: &'static str,
    pub leaf: &'static str,
    pub bar_full: char,
    pub bar_partial: &'static [char],
    pub bar_empty: char,
    pub spinner: &'static [&'static str],
}

impl Glyphs {
    pub const UNICODE: Self = Self {
        marked: "●",
        unmarked: "○",
        partial: "◐",
        locked: "⊘",
        expanded: "▾",
        collapsed: "▸",
        leaf: " ",
        bar_full: '█',
        bar_partial: &['▏', '▎', '▍', '▌', '▋', '▊', '▉'],
        bar_empty: ' ',
        spinner: &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
    };

    pub const ASCII: Self = Self {
        marked: "x",
        unmarked: ".",
        partial: "~",
        locked: "!",
        expanded: "v",
        collapsed: ">",
        leaf: " ",
        bar_full: '#',
        bar_partial: &[],
        bar_empty: ' ',
        spinner: &["|", "/", "-", "\\"],
    };

    pub fn named(name: &str) -> Self {
        match name {
            "ascii" => Self::ASCII,
            _ => Self::UNICODE,
        }
    }

    /// A proportional bar `width` cells wide.
    pub fn bar(&self, fraction: f64, width: usize) -> String {
        let fraction = fraction.clamp(0.0, 1.0);
        let eighths = (fraction * width as f64 * 8.0).round() as usize;
        let full = eighths / 8;
        let rest = eighths % 8;
        let mut bar = String::with_capacity(width * 3);
        for _ in 0..full.min(width) {
            bar.push(self.bar_full);
        }
        let mut len = full.min(width);
        if len < width && rest > 0 {
            match self.bar_partial.get(rest - 1) {
                Some(&c) => bar.push(c),
                None if rest >= 4 => bar.push(self.bar_full),
                None => bar.push(self.bar_empty),
            }
            len += 1;
        }
        for _ in len..width {
            bar.push(self.bar_empty);
        }
        bar
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn bars_have_fixed_width() {
        for glyphs in [Glyphs::UNICODE, Glyphs::ASCII] {
            for f in [0.0, 0.01, 0.33, 0.5, 0.99, 1.0, 2.0] {
                assert_eq!(glyphs.bar(f, 10).width(), 10, "{f}");
            }
        }
        assert_eq!(Glyphs::UNICODE.bar(1.0, 4), "████");
        assert_eq!(Glyphs::UNICODE.bar(0.0, 3), "   ");
    }

    #[test]
    fn themes_cycle() {
        let mut theme = Theme::ANSI;
        let mut seen = Vec::new();
        for _ in 0..Theme::NAMES.len() {
            seen.push(theme.name);
            theme = theme.next();
        }
        assert!(seen.contains(&"ansi"));
    }
}
