use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border::Set as BorderSet;

const ASCII_BORDER: BorderSet = BorderSet {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

#[derive(Clone, Copy)]
pub struct Theme {
    colors: bool,
    unicode: bool,
}

impl Theme {
    pub fn detect() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let dumb_terminal = std::env::var("TERM").is_ok_and(|term| term == "dumb");
        let unicode = ["LC_ALL", "LC_CTYPE", "LANG"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .any(|value| {
                let value = value.to_ascii_lowercase();
                value.contains("utf-8") || value.contains("utf8")
            });
        Self {
            colors: !no_color && !dumb_terminal,
            unicode,
        }
    }

    pub fn title(self) -> Style {
        self.semantic(Color::Cyan, Modifier::BOLD)
    }

    pub fn selected(self) -> Style {
        if self.colors {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    pub fn healthy(self) -> Style {
        self.semantic(Color::Green, Modifier::BOLD)
    }

    pub fn warning(self) -> Style {
        self.semantic(Color::Yellow, Modifier::BOLD)
    }

    pub fn error(self) -> Style {
        self.semantic(Color::Red, Modifier::BOLD)
    }

    pub fn unavailable(self) -> Style {
        self.semantic(Color::DarkGray, Modifier::ITALIC)
    }

    pub fn muted(self) -> Style {
        self.semantic(Color::Gray, Modifier::empty())
    }

    pub fn border(self) -> Style {
        self.semantic(Color::DarkGray, Modifier::empty())
    }

    pub fn border_set(self) -> BorderSet {
        if self.unicode {
            ratatui::symbols::border::PLAIN
        } else {
            ASCII_BORDER
        }
    }

    pub fn supports_unicode(self) -> bool {
        self.unicode
    }

    fn semantic(self, color: Color, modifier: Modifier) -> Style {
        if self.colors {
            Style::default().fg(color).add_modifier(modifier)
        } else {
            Style::default().add_modifier(modifier)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_theme_uses_ascii_borders_and_selected_text_without_color() {
        let theme = Theme {
            colors: false,
            unicode: false,
        };
        assert_eq!(theme.border_set().top_left, "+");
        assert_eq!(theme.border_set().horizontal_top, "-");
        assert_eq!(theme.border_set().vertical_left, "|");
        assert!(!theme.supports_unicode());
        assert!(theme.selected().fg.is_none());
        assert!(theme.selected().bg.is_none());
        assert!(theme.selected().add_modifier.contains(Modifier::BOLD));
    }
}
