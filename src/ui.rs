use crate::app::{App, ConfirmKind, InputKind, Mode};
use crate::model::COLUMNS;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

const ACCENT: Color = Color::Cyan;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),     // board
            Constraint::Length(12), // detail
            Constraint::Length(1),  // status / input line
        ])
        .split(f.area());

    draw_board(f, app, chunks[0]);
    draw_detail(f, app, chunks[1]);
    draw_bottom(f, app, chunks[2]);
}

fn draw_board(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, COLUMNS.len() as u32); COLUMNS.len()])
        .split(area);

    for (ci, name) in COLUMNS.iter().enumerate() {
        let indices = app.board.column_indices(ci);
        let selected_col = ci == app.sel_col;

        let items: Vec<ListItem> = indices
            .iter()
            .enumerate()
            .map(|(ri, &gi)| {
                let card = &app.board.cards[gi];
                let alive = app.live_sessions.contains(&card.session);
                let dot = if alive {
                    Span::styled("● ", Style::default().fg(Color::Green))
                } else {
                    Span::styled("○ ", Style::default().fg(Color::DarkGray))
                };
                let mut style = Style::default();
                if selected_col && ri == app.sel_row {
                    style = style.bg(Color::DarkGray).add_modifier(Modifier::BOLD);
                }
                ListItem::new(Line::from(vec![dot, Span::raw(card.title.clone())]))
                    .style(style)
            })
            .collect();

        let border_style = if selected_col {
            Style::default().fg(ACCENT)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let title_style = if selected_col {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(Span::styled(format!(" {} ({}) ", name, indices.len()), title_style));

        f.render_widget(List::new(items).block(block), cols[ci]);
    }
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    match app.selected_card() {
        Some(card) => {
            let alive = app.live_sessions.contains(&card.session);
            let state = if alive {
                Span::styled("running", Style::default().fg(Color::Green))
            } else {
                Span::styled("stopped", Style::default().fg(Color::DarkGray))
            };
            lines.push(Line::from(vec![
                Span::styled(&card.title, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("   "),
                state,
                Span::styled(
                    format!("   {}  $ {}", card.dir, card.command),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            lines.push(Line::from(Span::styled(
                format!("tmux: {}", card.session),
                Style::default().fg(Color::DarkGray),
            )));
            if app.tail.is_empty() {
                if !alive {
                    lines.push(Line::from(Span::styled(
                        "(no live session — Enter to start & attach, s to start detached)",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            } else {
                lines.push(Line::from(Span::styled(
                    "─ live output ─",
                    Style::default().fg(ACCENT),
                )));
                for l in &app.tail {
                    lines.push(Line::from(Span::raw(l.clone())));
                }
            }
        }
        None => {
            lines.push(Line::from(Span::styled(
                "empty column — press n to create a card",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" detail ");
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_bottom(f: &mut Frame, app: &App, area: Rect) {
    let line: Line = match &app.mode {
        Mode::Input { kind, buffer } => {
            let label = match kind {
                InputKind::NewTitle => "new card · title",
                InputKind::NewCommand { .. } => "new card · command",
                InputKind::NewDir { .. } => "new card · directory",
                InputKind::EditTitle { .. } => "edit title",
                InputKind::EditCommand { .. } => "edit command",
            };
            Line::from(vec![
                Span::styled(
                    format!(" {label}: "),
                    Style::default().fg(Color::Black).bg(ACCENT),
                ),
                Span::raw(format!(" {buffer}")),
                Span::styled("▏", Style::default().fg(ACCENT)),
                Span::styled("  (Enter confirm · Esc cancel)", Style::default().fg(Color::DarkGray)),
            ])
        }
        Mode::Confirm(kind) => {
            let (msg, title) = match kind {
                ConfirmKind::DeleteCard { .. } => {
                    let extra = app
                        .selected_card()
                        .map(|c| app.live_sessions.contains(&c.session))
                        .unwrap_or(false);
                    if extra {
                        ("delete card AND kill its live session?", "y/n")
                    } else {
                        ("delete card?", "y/n")
                    }
                }
                ConfirmKind::KillSession { .. } => ("kill this card's tmux session?", "y/n"),
            };
            Line::from(vec![
                Span::styled(
                    format!(" {msg} "),
                    Style::default().fg(Color::Black).bg(Color::Yellow),
                ),
                Span::styled(format!(" [{title}]"), Style::default().fg(Color::Yellow)),
            ])
        }
        Mode::Normal => Line::from(vec![
            Span::styled(format!(" {} ", app.status), Style::default().fg(Color::White)),
            Span::styled(
                "  hjkl:move  [/]:move card  J/K:reorder  n:new  Enter:attach  s:start  x:kill  o:notes  e/c:edit  d:del  q:quit",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    };
    f.render_widget(Paragraph::new(line), area);
}
