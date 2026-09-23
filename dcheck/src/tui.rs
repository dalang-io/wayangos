//! Terminal UI (ratatui). Entered for interactive use; falls back to the plain
//! text menu when stdout is not a terminal.

use std::io::{self, Stdout};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::{Frame, Terminal};

use crate::model::Device;
use crate::report;

const GOLD: Color = Color::Rgb(200, 148, 26);

#[derive(Clone, Copy)]
enum Screen {
    Menu,
    Storage,
    Report,
}

pub struct App {
    devices: Vec<Device>,
    screen: Screen,
    menu: ListState,
    table: TableState,
    report_lines: Vec<String>,
    scroll: u16,
}

impl App {
    fn new(devices: Vec<Device>) -> Self {
        let mut menu = ListState::default();
        menu.select(Some(0));
        let mut table = TableState::default();
        if !devices.is_empty() {
            table.select(Some(0));
        }
        App {
            devices,
            screen: Screen::Menu,
            menu,
            table,
            report_lines: Vec::new(),
            scroll: 0,
        }
    }

    fn menu_items() -> [&'static str; 4] {
        [
            "Storage",
            "RAM            (coming soon)",
            "CPU            (coming soon)",
            "Quit",
        ]
    }

    fn open_report(&mut self) {
        if let Some(i) = self.table.selected() {
            if let Some(dev) = self.devices.get(i) {
                self.report_lines = report::device_report_lines(dev);
                self.scroll = 0;
                self.screen = Screen::Report;
            }
        }
    }
}

/// Run the TUI. Must be called on a real terminal.
pub fn run(devices: Vec<Device>) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, devices);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(terminal: &mut Terminal<ratatui::backend::CrosstermBackend<Stdout>>, devices: Vec<Device>) -> io::Result<()> {
    let mut app = App::new(devices);
    loop {
        terminal.draw(|f| draw(f, &mut app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match app.screen {
                Screen::Menu => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Up | KeyCode::Char('k') => {
                        let i = app.menu.selected().unwrap_or(0);
                        app.menu.select(Some(i.saturating_sub(1)));
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let i = app.menu.selected().unwrap_or(0);
                        app.menu.select(Some((i + 1).min(3)));
                    }
                    KeyCode::Enter => match app.menu.selected().unwrap_or(0) {
                        0 => app.screen = Screen::Storage,
                        1 | 2 => {
                            // coming soon: no-op (label already says so)
                        }
                        _ => break,
                    },
                    _ => {}
                },
                Screen::Storage => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => app.screen = Screen::Menu,
                    KeyCode::Up | KeyCode::Char('k') => {
                        let i = app.table.selected().unwrap_or(0);
                        app.table.select(Some(i.saturating_sub(1)));
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let i = app.table.selected().unwrap_or(0);
                        let max = app.devices.len().saturating_sub(1);
                        app.table.select(Some((i + 1).min(max)));
                    }
                    KeyCode::Enter => app.open_report(),
                    _ => {}
                },
                Screen::Report => match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('b') => {
                        app.screen = Screen::Storage;
                    }
                    KeyCode::Down | KeyCode::Char('j') => app.scroll = app.scroll.saturating_add(1),
                    KeyCode::Up | KeyCode::Char('k') => app.scroll = app.scroll.saturating_sub(1),
                    KeyCode::PageDown => app.scroll = app.scroll.saturating_add(10),
                    KeyCode::PageUp => app.scroll = app.scroll.saturating_sub(10),
                    _ => {}
                },
            }
        }
    }
    Ok(())
}

fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(f.area());

    let title = Paragraph::new(Line::from(vec![
        ratatui::text::Span::styled(
            "dcheck",
            Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
        ),
        ratatui::text::Span::raw("  — device health check"),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    match app.screen {
        Screen::Menu => {
            let items: Vec<ListItem> = App::menu_items()
                .iter()
                .map(|s| ListItem::new(*s))
                .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Menu "))
                .highlight_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(GOLD)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("> ");
            f.render_stateful_widget(list, chunks[1], &mut app.menu);
            hint(f, chunks[2], "↑/↓ move   Enter select   q quit");
        }
        Screen::Storage => {
            let rows: Vec<Row> = app
                .devices
                .iter()
                .map(|d| {
                    Row::new(vec![
                        d.path.clone(),
                        d.kind.to_string(),
                        d.bus.to_string(),
                        d.label(),
                        report::human_size(d.size_bytes),
                        report::mount_summary(d),
                    ])
                })
                .collect();
            let header = Row::new(vec!["DEVICE", "TYPE", "BUS", "MODEL", "CAPACITY", "MOUNT"])
                .style(Style::default().fg(GOLD).add_modifier(Modifier::BOLD));
            let widths = [
                Constraint::Length(14),
                Constraint::Length(5),
                Constraint::Length(7),
                Constraint::Min(20),
                Constraint::Length(10),
                Constraint::Length(16),
            ];
            let table = Table::new(rows, widths)
                .header(header)
                .block(Block::default().borders(Borders::ALL).title(" Storage "))
                .row_highlight_style(Style::default().bg(Color::Rgb(40, 32, 16)).fg(GOLD))
                .highlight_symbol("> ");
            f.render_stateful_widget(table, chunks[1], &mut app.table);
            hint(f, chunks[2], "↑/↓ move   Enter report   Esc back   q quit");
        }
        Screen::Report => {
            let lines: Vec<Line> = app
                .report_lines
                .iter()
                .map(|l| Line::from(l.clone()))
                .collect();
            let para = Paragraph::new(Text::from(lines))
                .block(Block::default().borders(Borders::ALL).title(" Report "))
                .wrap(Wrap { trim: false })
                .scroll((app.scroll, 0));
            f.render_widget(para, chunks[1]);
            hint(f, chunks[2], "↑/↓ or PgUp/PgDn scroll   b/Esc back   q quit");
        }
    }
}

fn hint(f: &mut Frame, area: ratatui::layout::Rect, text: &str) {
    let p = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(p, area);
}
