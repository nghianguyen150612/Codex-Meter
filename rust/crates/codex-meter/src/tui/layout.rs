use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub struct FrameLayout {
    pub header: Rect,
    pub tabs: Option<Rect>,
    pub sidebar: Option<Rect>,
    pub content: Rect,
    pub footer: Rect,
    pub wide: bool,
}

pub fn responsive_layout(area: Rect) -> FrameLayout {
    let height = area.height;
    let header_height = height.min(2);
    let footer_height = if height >= 4 {
        2
    } else if height >= 3 {
        1
    } else {
        0
    };
    let wide = area.width >= 100 && height >= 10;
    let tabs_height = if wide || height < 6 { 0 } else { 1 };
    let middle_height = height
        .saturating_sub(header_height)
        .saturating_sub(footer_height)
        .saturating_sub(tabs_height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Length(tabs_height),
            Constraint::Length(middle_height),
            Constraint::Length(footer_height),
        ])
        .split(area);
    let header = chunks[0];
    let tabs = (tabs_height > 0).then_some(chunks[1]);
    let footer = chunks[3];
    let middle = chunks[2];
    if wide && middle.width >= 40 {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(21), Constraint::Min(0)])
            .split(middle);
        FrameLayout {
            header,
            tabs,
            sidebar: Some(columns[0]),
            content: columns[1],
            footer,
            wide,
        }
    } else {
        FrameLayout {
            header,
            tabs,
            sidebar: None,
            content: middle,
            footer,
            wide: false,
        }
    }
}
