use ratatui::style::Color;

// Catppuccin Mocha, the herdr default palette.
pub struct Theme {
    pub bg: Color,
    pub text: Color,
    pub sub: Color,
    pub dim: Color,
    pub line: Color,
    pub accent: Color,
    pub sel_fg: Color,
    pub green: Color,
    pub yellow: Color,
    pub teal: Color,
    pub red: Color,
}

pub const MOCHA: Theme = Theme {
    bg: Color::Rgb(30, 30, 46),
    text: Color::Rgb(205, 214, 244),
    sub: Color::Rgb(166, 173, 200),
    dim: Color::Rgb(108, 112, 134),
    line: Color::Rgb(69, 71, 90),
    accent: Color::Rgb(137, 180, 250),
    sel_fg: Color::Rgb(17, 17, 27),
    green: Color::Rgb(166, 227, 161),
    yellow: Color::Rgb(249, 226, 175),
    teal: Color::Rgb(148, 226, 213),
    red: Color::Rgb(243, 139, 168),
};
