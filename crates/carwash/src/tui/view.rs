//! Rendering. Pure functions of [`App`], except for recording table geometry for the mouse.

use super::app::{App, Hotspot, Level, Mode, Review, ReviewPhase, Target};
use super::keymap::{self, Binding, Section, Tab};
use super::store::{EntryStatus, Row, RowKey};
use carwash_core::clean::DeleteMode;
use carwash_core::select::Hold;
use carwash_core::{Detection, GitState, fmt};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Gauge, Paragraph, Row as TableRow, Table, TableState,
    Wrap,
};
use unicode_width::UnicodeWidthStr;

const DETAILS_WIDTH: u16 = 46;
const MIN_WIDTH_FOR_DETAILS: u16 = 112;
const MIN_WIDTH_FOR_BARS: u16 = 96;

pub fn render(frame: &mut Frame, app: &mut App) {
    app.ensure_rows();
    let [header, toolbar, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    render_header(frame, app, header);
    render_toolbar(frame, app, toolbar);
    let mut hotspots = tab_hotspots(app, toolbar);
    let details = app.show_details && body.width >= MIN_WIDTH_FOR_DETAILS;
    if app.tab == Tab::Tasks {
        super::tasks::render(frame, app, body);
    } else if app.tab == Tab::Updates {
        super::updates::render(frame, app, body);
    } else if app.tab == Tab::Caches {
        super::caches::render(frame, app, body);
    } else if details {
        let [table, side] =
            Layout::horizontal([Constraint::Min(60), Constraint::Length(DETAILS_WIDTH)])
                .areas(body);
        render_table(frame, app, table);
        render_details(frame, app, side);
    } else {
        render_table(frame, app, body);
    }
    hotspots.extend(render_footer(frame, app, footer));
    app.hotspots = hotspots;

    match &app.mode {
        Mode::Review(review) => render_review(frame, app, review),
        Mode::Help => render_help(frame, app),
        Mode::Browse => render_tooltip(frame, app),
        Mode::Folder => render_folder_candidates(frame, app, footer),
        Mode::Search => {}
    }
}

/// Folders matching the prompt, just above it.
fn render_folder_candidates(frame: &mut Frame, app: &App, footer: Rect) {
    const SHOWN: usize = 12;
    let t = &app.theme;
    let input = app.input.value();
    let candidates = app.folder.for_input(input);
    if candidates.is_empty() {
        return;
    }
    let parent_len = input.rfind('/').map_or(0, |i| i + 1);
    let mut lines: Vec<Line> = candidates
        .iter()
        .take(SHOWN)
        .map(|c| {
            let name = c.get(parent_len..).unwrap_or(c);
            Line::from(Span::styled(name.to_owned(), t.text()))
        })
        .collect();
    if candidates.len() > SHOWN {
        lines.push(Line::from(Span::styled(
            format!("+{} more, keep typing", candidates.len() - SHOWN),
            t.muted(),
        )));
    }
    let width = lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4;
    let height = lines.len() as u16 + 2;
    let x = footer.x + " folder ".len() as u16 + parent_len as u16;
    let popup = Rect::new(
        x.min(footer.right().saturating_sub(width)),
        footer.y.saturating_sub(height),
        width,
        height,
    )
    .intersection(frame.area());
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(t.fg(t.accent))
                .padding(ratatui::widgets::Padding::horizontal(1)),
        ),
        popup,
    );
}

pub(super) fn home_relative(path: &std::path::Path) -> String {
    if let Some(home) = carwash_core::paths::home()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

use super::widgets::spinner;

/// Renders `left` and right-aligned `right` on one line; `right` wins when space runs out.
fn split_line(frame: &mut Frame, area: Rect, left: Line<'_>, right: Line<'_>) {
    let right_width = (right.width() as u16).min(area.width);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_width)]).areas(area);
    frame.render_widget(Paragraph::new(left), left_area);
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        right_area,
    );
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let left = Line::from(vec![
        Span::styled(
            " carwash ",
            t.bold(t.accent2).add_modifier(Modifier::REVERSED),
        ),
        Span::raw(" "),
        Span::styled(home_relative(&app.store.root), t.bold(t.text)),
    ]);

    let mut right: Vec<Span> = Vec::new();
    if let Some(progress) = app.scan_progress() {
        right.push(Span::styled(format!("{} ", spinner(app)), t.fg(t.accent)));
        right.push(Span::styled(progress, t.subtle()));
    } else {
        let totals = app.rows.totals;
        right.push(Span::styled(fmt::bytes(totals.reclaimable), t.bold(t.text)));
        right.push(Span::styled(" reclaimable", t.subtle()));
        if let Some(elapsed) = app.scan.elapsed {
            right.push(Span::styled(
                format!(" · scanned in {:.1}s", elapsed.as_secs_f64()),
                t.muted(),
            ));
        }
    }
    if let Some((free, total)) = app.disk {
        right.push(Span::styled("  │  ", t.muted()));
        right.push(Span::styled(fmt::bytes(free), t.bold(t.text)));
        right.push(Span::styled(
            format!(" free of {} ", fmt::bytes(total)),
            t.subtle(),
        ));
    }
    split_line(frame, area, left, Line::from(right));
}

/// The tab titles, each followed by a space, after a leading space.
fn tab_spans(app: &App) -> Vec<Span<'static>> {
    let t = &app.theme;
    let mut spans = vec![Span::raw(" ")];
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let style = if *tab == app.tab {
            t.bold(t.accent).add_modifier(Modifier::REVERSED)
        } else {
            t.muted()
        };
        spans.push(Span::styled(format!(" {} {} ", i + 1, tab.title()), style));
        spans.push(Span::raw(" "));
    }
    spans
}

fn tab_hotspots(app: &App, area: Rect) -> Vec<Hotspot> {
    let spans = tab_spans(app);
    let mut x = area.x + spans[0].width() as u16;
    let mut out = Vec::new();
    for (tab, pair) in Tab::ALL.iter().zip(spans[1..].chunks(2)) {
        let width = pair[0].width() as u16;
        out.push(Hotspot {
            area: Rect::new(x, area.y, width, 1).intersection(area),
            target: Target::Tab(*tab),
        });
        x += width + pair.get(1).map_or(0, |s| s.width() as u16);
    }
    out
}

fn render_toolbar(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let mut left = tab_spans(app);
    if app.tab == Tab::Tasks {
        let running = app.tasks.running();
        let mut right = vec![Span::styled(
            format!("{} jobs", app.tasks.jobs.len()),
            t.muted(),
        )];
        if running > 0 {
            right.push(Span::styled(
                format!(" · {running} running "),
                t.fg(t.accent),
            ));
        } else {
            right.push(Span::raw(" "));
        }
        split_line(frame, area, Line::from(left), Line::from(right));
        return;
    }
    if app.tab == Tab::Caches {
        let right = Line::from(vec![
            Span::styled(fmt::bytes(app.caches.total()), t.bold(t.text)),
            Span::styled(" in caches ", t.muted()),
        ]);
        split_line(frame, area, Line::from(left), right);
        return;
    }
    if app.tab == Tab::Updates {
        let (outdated, vulnerable) = app.updates.totals();
        let mut right = vec![Span::styled(
            format!("{} checked", app.updates.results.len()),
            t.muted(),
        )];
        if outdated > 0 {
            right.push(Span::styled(
                format!(" · {outdated} outdated"),
                t.fg(t.warning),
            ));
        }
        if vulnerable > 0 {
            right.push(Span::styled(
                format!(" · {vulnerable} vulnerable"),
                t.bold(t.error),
            ));
        }
        right.push(Span::raw(" "));
        split_line(frame, area, Line::from(left), Line::from(right));
        return;
    }
    left.extend([
        Span::styled(" view ", t.muted()),
        Span::styled(app.grouping.label(), t.fg(t.accent)),
        Span::styled("  sort ", t.muted()),
        Span::styled(app.sort.label(), t.fg(t.accent)),
    ]);
    if !app.query.is_empty() && !matches!(app.mode, Mode::Search) {
        left.push(Span::styled("  filter ", t.muted()));
        left.push(Span::styled(app.query.raw.clone(), t.fg(t.warning)));
        left.push(Span::styled(
            format!("  {} shown", app.rows.totals.artifacts),
            t.muted(),
        ));
    }
    let ready_bytes: u64 = app
        .rows
        .order
        .iter()
        .filter_map(|id| app.store.entry(*id))
        .filter(|e| {
            e.status == EntryStatus::Present
                && super::store::hold_for(e, &app.policy, app.now).is_none()
        })
        .filter_map(|e| e.size())
        .map(|s| s.reclaimable)
        .sum();
    let marked_bytes = app.marked_bytes();
    let mut right = vec![
        Span::styled(fmt::bytes(ready_bytes), t.fg(t.success)),
        Span::styled(" ready", t.muted()),
    ];
    if !app.marked.is_empty() {
        right.push(Span::styled("  │  ", t.muted()));
        right.push(Span::styled(
            format!(
                "{} marked · {} ",
                app.marked.len(),
                fmt::bytes(marked_bytes)
            ),
            t.bold(t.accent2),
        ));
    } else {
        right.push(Span::raw(" "));
    }
    split_line(frame, area, Line::from(left), Line::from(right));
}

fn badge<'a>(app: &'a App, eco: carwash_core::EcoId) -> Span<'a> {
    let ecosystem = app.registry.ecosystem(eco);
    Span::styled(
        ecosystem.badge.as_str(),
        Style::new().fg(app.theme.ecosystem(&ecosystem.key)),
    )
}

fn mark_cell(app: &App, row: &Row) -> Span<'static> {
    let t = &app.theme;
    let g = &app.glyphs;
    if let RowKey::Artifact(id) = row.key {
        let entry = app.store.entry(id);
        if entry.is_some_and(|e| e.status == EntryStatus::Deleting) {
            return Span::styled(spinner(app), t.fg(t.accent));
        }
        return if app.marked.contains(&id) {
            Span::styled(g.marked, t.bold(t.accent2))
        } else if row.totals.markable == 0 {
            Span::styled(g.locked, t.fg(t.error))
        } else {
            Span::styled(g.unmarked, t.muted())
        };
    }
    let totals = row.totals;
    if totals.markable == 0 {
        Span::styled(g.locked, t.muted())
    } else if totals.marked == 0 {
        Span::styled(g.unmarked, t.muted())
    } else if totals.marked >= totals.ready.max(1) {
        Span::styled(g.marked, t.bold(t.accent2))
    } else {
        Span::styled(g.partial, t.fg(t.accent2))
    }
}

fn status_cell(app: &App, row: &Row) -> Span<'static> {
    let t = &app.theme;
    match row.key {
        RowKey::Artifact(id) => {
            let Some(entry) = app.store.entry(id) else {
                return Span::raw("");
            };
            match &entry.status {
                EntryStatus::Deleting => return Span::styled("deleting", t.muted()),
                EntryStatus::Failed(_) => return Span::styled("failed", t.bold(t.error)),
                _ => {}
            }
            match super::store::hold_for(entry, &app.policy, app.now) {
                None => Span::styled("ready", t.fg(t.success)),
                Some(Hold::Recent) => Span::styled("recent", t.fg(t.info)),
                Some(Hold::Review) => Span::styled("review", t.fg(t.warning)),
                Some(Hold::Protected) => Span::styled("protected", t.fg(t.error)),
            }
        }
        _ => {
            let totals = row.totals;
            let held = totals.markable.saturating_sub(totals.ready);
            if totals.marked > 0 {
                Span::styled(format!("{} marked", totals.marked), t.fg(t.accent2))
            } else if held > 0 {
                Span::styled(format!("{held} held"), t.muted())
            } else {
                Span::raw("")
            }
        }
    }
}

fn size_cell(app: &App, row: &Row) -> Span<'static> {
    let t = &app.theme;
    let totals = row.totals;
    if totals.artifacts == totals.unmeasured {
        return Span::styled(if app.scan.running { spinner(app) } else { "?" }, t.muted());
    }
    let text = fmt::bytes(totals.reclaimable);
    if totals.estimated || (totals.unmeasured > 0 && app.scan.running) {
        Span::styled(format!("~{text}"), t.subtle())
    } else {
        Span::styled(text, t.size(totals.reclaimable))
    }
}

fn render_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.fg(t.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.rows.rows.is_empty() {
        let message = if app.scan.running {
            format!(
                "{} {}",
                spinner(app),
                app.scan_progress().unwrap_or_default()
            )
        } else if !app.query.is_empty() {
            format!("No artifacts match “{}”", app.query.raw)
        } else {
            "Nothing to reclaim here.".to_string()
        };
        let [middle] = Layout::vertical([Constraint::Length(1)])
            .flex(Flex::Center)
            .areas(inner);
        frame.render_widget(
            Paragraph::new(message)
                .alignment(Alignment::Center)
                .style(t.subtle()),
            middle,
        );
        return;
    }

    let bars = inner.width >= MIN_WIDTH_FOR_BARS;
    let max = app.rows.max_top.max(1);
    let glyphs = app.glyphs;
    let rows: Vec<TableRow> = app
        .rows
        .rows
        .iter()
        .map(|row| {
            let indent = "  ".repeat(usize::from(row.depth));
            let expander = if row.expandable {
                if row.expanded {
                    glyphs.expanded
                } else {
                    glyphs.collapsed
                }
            } else {
                glyphs.leaf
            };
            let name_style = match row.key {
                RowKey::Project(_) => t.bold(t.text),
                RowKey::Artifact(_) => t.text(),
                _ => t.fg(t.subtle),
            };
            let mut name = vec![
                Span::raw(indent),
                Span::styled(format!("{expander} "), t.muted()),
                Span::styled(row.label.clone(), name_style),
            ];
            let kind = match row.key {
                RowKey::Artifact(id) => app
                    .store
                    .entry(id)
                    .map(|e| Span::styled(e.artifact.kind.label(), t.subtle()))
                    .unwrap_or_default(),
                _ => {
                    if row.totals.artifacts > 1 {
                        name.push(Span::styled(
                            format!("  {}", row.totals.artifacts),
                            t.muted(),
                        ));
                    }
                    Span::raw("")
                }
            };
            let name = Line::from(name);
            let mut ecos: Vec<Span> = Vec::new();
            for (i, &eco) in row.ecosystems.iter().take(2).enumerate() {
                if i > 0 {
                    ecos.push(Span::raw(" "));
                }
                ecos.push(badge(app, eco));
            }
            let age = super::store::age(row.newest, app.now)
                .map(fmt::age)
                .unwrap_or_default();
            let mut cells = vec![
                Cell::from(mark_cell(app, row)),
                Cell::from(name),
                Cell::from(kind),
                Cell::from(Line::from(ecos)),
                Cell::from(status_cell(app, row)),
                Cell::from(Line::from(size_cell(app, row)).alignment(Alignment::Right)),
            ];
            if bars {
                let fraction = row.totals.reclaimable as f64 / max as f64;
                cells.push(Cell::from(Span::styled(
                    glyphs.bar(fraction, 10),
                    t.size(row.totals.reclaimable),
                )));
            }
            cells.push(Cell::from(
                Line::from(Span::styled(age, t.muted())).alignment(Alignment::Right),
            ));
            TableRow::new(cells)
        })
        .collect();

    let mut widths = vec![
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Length(9),
        Constraint::Length(9),
    ];
    if bars {
        widths.push(Constraint::Length(10));
    }
    widths.push(Constraint::Length(4));

    let mut header_cells = vec!["", "NAME", "KIND", "ECO", "STATUS", "FREED"];
    if bars {
        header_cells.push("");
    }
    header_cells.push("AGE");
    let header = TableRow::new(header_cells.into_iter().enumerate().map(|(i, h)| {
        let line = Line::from(Span::styled(h, t.bold(t.muted)));
        Cell::from(if i == 5 || h == "AGE" {
            line.alignment(Alignment::Right)
        } else {
            line
        })
    }));

    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .row_highlight_style(t.selected_row());

    let mut state = TableState::default()
        .with_offset(app.geometry.offset)
        .with_selected(Some(app.selected));
    frame.render_stateful_widget(table, inner, &mut state);
    app.geometry = super::app::TableGeometry {
        area: inner,
        first_row_y: inner.y + 1,
        offset: state.offset(),
    };
    app.page = usize::from(inner.height.saturating_sub(2)).max(1);
}

fn kv<'a>(app: &App, key: &'a str, value: impl Into<String>) -> Line<'a> {
    let t = &app.theme;
    Line::from(vec![
        Span::styled(format!("{key:<10}"), t.muted()),
        Span::styled(value.into(), t.text()),
    ])
}

fn render_details(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.fg(t.border))
        .title(Span::styled(" Details ", t.muted()));
    let mut lines: Vec<Line> = Vec::new();
    match app.selected_row().map(|r| (r.key.clone(), r.totals)) {
        Some((RowKey::Artifact(id), _)) => {
            if let Some(entry) = app.store.entry(id) {
                let a = &entry.artifact;
                lines.push(Line::from(Span::styled(
                    app.store.relative(&a.path).into_owned(),
                    t.bold(t.text),
                )));
                lines.push(Line::default());
                if let Some(project) = a.project.and_then(|p| app.store.project(p)) {
                    lines.push(kv(app, "project", project.name.clone()));
                }
                if let Some(eco) = a.ecosystem {
                    lines.push(kv(
                        app,
                        "ecosystem",
                        app.registry.ecosystem(eco).name.clone(),
                    ));
                }
                lines.push(kv(app, "kind", a.kind.label()));
                lines.push(kv(app, "detected", detection(app, a.detection)));
                lines.push(kv(app, "git", git_state(a.git)));
                let hold = super::store::hold_for(entry, &app.policy, app.now);
                let (status, color) = match (a.safety(), hold) {
                    (carwash_core::Safety::Protected(_), _) => {
                        (a.safety().describe().to_string(), t.error)
                    }
                    (carwash_core::Safety::Review(_), _) => {
                        (a.safety().describe().to_string(), t.warning)
                    }
                    (_, Some(Hold::Recent)) => ("used recently; held back".to_string(), t.info),
                    _ => ("ready to clean".to_string(), t.success),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{:<10}", "status"), t.muted()),
                    Span::styled(status, t.fg(color)),
                ]));
                lines.push(Line::default());
                match entry.size() {
                    Some(size) => {
                        let prefix = if entry.is_estimate() { "~" } else { "" };
                        lines.push(kv(
                            app,
                            "frees",
                            format!("{prefix}{}", fmt::bytes(size.reclaimable)),
                        ));
                        lines.push(kv(app, "on disk", fmt::bytes(size.on_disk)));
                        if size.shared() > 0 {
                            lines.push(Line::from(vec![
                                Span::styled(format!("{:<10}", "shared"), t.muted()),
                                Span::styled(
                                    format!("{} hard-linked elsewhere", fmt::bytes(size.shared())),
                                    t.fg(t.warning),
                                ),
                            ]));
                        }
                        lines.push(kv(
                            app,
                            "contents",
                            format!(
                                "{} files · {} dirs",
                                crate::commands::count(size.files),
                                crate::commands::count(size.dirs)
                            ),
                        ));
                        if size.errors > 0 {
                            lines.push(kv(app, "unreadable", size.errors.to_string()));
                        }
                    }
                    None => lines.push(kv(app, "frees", "measuring…")),
                }
                if let Some(age) = super::store::age(entry.last_modified(), app.now) {
                    lines.push(kv(app, "modified", format!("{} ago", fmt::age(age))));
                }
                if let Detection::Rule { rule, .. } = a.detection
                    && let Some(regenerate) = &app.registry.rule(rule).regenerate
                {
                    lines.push(Line::default());
                    lines.push(Line::from(Span::styled("regenerate with", t.muted())));
                    lines.push(Line::from(Span::styled(regenerate.clone(), t.subtle())));
                }
                if let EntryStatus::Failed(error) = &entry.status {
                    lines.push(Line::default());
                    lines.push(Line::from(Span::styled(error.clone(), t.fg(t.error))));
                }
            }
        }
        Some((RowKey::Project(id), totals)) => {
            if let Some(project) = app.store.project(id) {
                let relative = app.store.relative(&project.path).into_owned();
                lines.push(Line::from(Span::styled(
                    project.name.clone(),
                    t.bold(t.text),
                )));
                if relative != project.name {
                    lines.push(Line::from(Span::styled(relative, t.subtle())));
                }
                lines.push(Line::default());
                let ecos: Vec<String> = project
                    .ecosystems
                    .iter()
                    .map(|&e| app.registry.ecosystem(e).name.clone())
                    .collect();
                lines.push(kv(app, "ecosystem", ecos.join(", ")));
                if project.is_workspace {
                    lines.push(kv(app, "workspace", "yes (declares members)"));
                }
                if let Some(ws) = project.member_of.and_then(|w| app.store.project(w)) {
                    lines.push(kv(app, "member of", ws.name.clone()));
                }
                if let Some(repo) = &project.repo {
                    lines.push(kv(app, "repo", home_relative(repo)));
                }
                if let Some(age) = super::store::age(project.last_activity, app.now) {
                    lines.push(kv(
                        app,
                        "manifest",
                        format!("changed {} ago", fmt::age(age)),
                    ));
                }
                if project.outside_root {
                    lines.push(kv(app, "note", "encloses the scan root"));
                }
                lines.push(Line::default());
                push_totals(app, &mut lines, totals);
            }
        }
        Some((RowKey::Dir(path), totals)) => {
            lines.push(Line::from(Span::styled(
                app.store.relative(&path).into_owned(),
                t.bold(t.text),
            )));
            lines.push(Line::default());
            push_totals(app, &mut lines, totals);
        }
        Some((RowKey::Orphans, totals)) => {
            lines.push(Line::from(Span::styled(
                "Outside any project",
                t.bold(t.text),
            )));
            lines.push(Line::from(Span::styled(
                "Recognised by content (venvs, caches, stray node_modules).",
                t.subtle(),
            )));
            lines.push(Line::default());
            push_totals(app, &mut lines, totals);
        }
        None => {}
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn push_totals(app: &App, lines: &mut Vec<Line<'_>>, totals: super::store::Totals) {
    lines.push(kv(app, "artifacts", totals.artifacts.to_string()));
    lines.push(kv(app, "frees", fmt::bytes(totals.reclaimable)));
    if totals.on_disk != totals.reclaimable {
        lines.push(kv(app, "on disk", fmt::bytes(totals.on_disk)));
    }
    lines.push(kv(
        app,
        "ready",
        format!("{} of {}", totals.ready, totals.markable),
    ));
    if totals.marked > 0 {
        lines.push(kv(
            app,
            "marked",
            format!("{} · {}", totals.marked, fmt::bytes(totals.marked_bytes)),
        ));
    }
}

fn detection(app: &App, detection: Detection) -> String {
    match detection {
        Detection::Rule { rule, confirmed } => {
            let rule = app.registry.rule(rule);
            if confirmed {
                format!("rule `{}`, confirmed by contents", rule.path)
            } else {
                format!("rule `{}`", rule.path)
            }
        }
        Detection::CacheDirTag => "CACHEDIR.TAG".into(),
        Detection::PythonVenv => "pyvenv.cfg (virtualenv)".into(),
        Detection::CondaEnv => "conda-meta (conda env)".into(),
        Detection::CMakeBuild => "CMakeCache.txt".into(),
        Detection::MesonBuild => "Meson build directory".into(),
        Detection::CargoFingerprint => "Cargo fingerprints".into(),
        Detection::CarwashLeftover => "interrupted carwash clean".into(),
    }
}

fn git_state(state: GitState) -> String {
    match state {
        GitState::Unknown => "checking…".into(),
        GitState::NotInRepo => "not in a repository".into(),
        GitState::Ignored => "ignored".into(),
        GitState::Untracked => "untracked, not ignored".into(),
        GitState::Tracked(n) => format!("{n} tracked files"),
    }
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) -> Vec<Hotspot> {
    let t = &app.theme;
    let prompt = match app.mode {
        Mode::Search => Some(("/ ", "")),
        Mode::Folder => Some((" folder ", "Tab completes · Enter scans · Esc cancels ")),
        _ => None,
    };
    if let Some((prompt, help)) = prompt {
        let help_width = (help.width() as u16).min(area.width / 2);
        let [input_area, help_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(help_width)]).areas(area);
        let width = input_area.width.saturating_sub(prompt.width() as u16 + 1) as usize;
        let scroll = app.input.visual_scroll(width);
        let line = Line::from(vec![
            Span::styled(prompt, t.bold(t.accent)),
            Span::styled(
                app.input.value().chars().skip(scroll).collect::<String>(),
                t.text(),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), input_area);
        frame.render_widget(Paragraph::new(Span::styled(help, t.muted())), help_area);
        let cursor = (app.input.visual_cursor().saturating_sub(scroll)) as u16;
        frame.set_cursor_position((area.x + prompt.width() as u16 + cursor, area.y));
        return Vec::new();
    }
    if let Some(toast) = &app.toast {
        let color = match toast.level {
            Level::Info => t.accent,
            Level::Warn => t.warning,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {}", toast.text),
                t.fg(color),
            ))),
            area,
        );
        return Vec::new();
    }
    // This tab's actions on the left, as many as fit; the keys that work everywhere on the
    // right, labelled as such.
    const GAP: u16 = 2;
    /// Room left for this tab's keys, at the least.
    const MIN_LOCAL: u16 = 20;
    let (local, mut global) = keymap::footer(app.tab);
    let width = |b: &Binding| (b.key().width() + 1 + b.hint.unwrap_or_default().width()) as u16;
    let item = |b: &'static Binding, key_style: Style| {
        [
            Span::styled(b.key(), key_style),
            Span::styled(format!(" {}", b.hint.unwrap_or_default()), t.muted()),
        ]
    };
    let group_width = |global: &[&'static Binding], label: &str| {
        label.width() as u16
            + global.iter().map(|b| width(b)).sum::<u16>()
            + GAP * (global.len() as u16).saturating_sub(1)
            + 1
    };
    // When space runs short, help and quit outlast the other global keys, and the label goes.
    let mut label = "│ any tab  ";
    if group_width(&global, label) + MIN_LOCAL > area.width {
        global.retain(|b| matches!(b.action, keymap::Action::Help | keymap::Action::Quit));
    }
    if group_width(&global, label) + MIN_LOCAL > area.width {
        label = "│ ";
    }
    let global_width = group_width(&global, label);
    let show_global = global_width + MIN_LOCAL <= area.width;
    let limit = if show_global {
        area.right().saturating_sub(global_width + GAP)
    } else {
        area.right()
    };

    let mut hotspots = Vec::new();
    let mut spans = vec![Span::raw(" ")];
    let mut x = area.x + 1;
    for b in local {
        let start = if hotspots.is_empty() { x } else { x + GAP };
        if start + width(b) > limit {
            break;
        }
        if !hotspots.is_empty() {
            spans.push(Span::raw("  "));
        }
        spans.extend(item(b, t.bold(t.accent)));
        hotspots.push(Hotspot {
            area: Rect::new(start, area.y, width(b), 1),
            target: Target::Binding(b),
        });
        x = start + width(b);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    if show_global {
        let right_area = Rect::new(area.right() - global_width, area.y, global_width, 1);
        let mut spans = vec![Span::styled(label, t.muted())];
        let mut x = right_area.x + label.width() as u16;
        for (i, b) in global.into_iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
                x += GAP;
            }
            spans.extend(item(b, t.bold(t.subtle)));
            hotspots.push(Hotspot {
                area: Rect::new(x, area.y, width(b), 1),
                target: Target::Binding(b),
            });
            x += width(b);
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), right_area);
    }
    hotspots
}

/// Explains the hovered tab or footer key, above the footer or below the tab bar.
fn render_tooltip(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let Some(hover) = app.hover else {
        return;
    };
    // The hotspot may have moved or gone since the mouse last moved.
    let Some(spot) = app.hotspots.iter().find(|h| h.area == hover.area) else {
        return;
    };
    let (title, mut lines) = match spot.target {
        Target::Tab(tab) => {
            let key = Tab::ALL.iter().position(|t| *t == tab).unwrap_or(0) + 1;
            (
                format!("{} · press {key}", tab.title()),
                vec![Line::from(Span::styled(tab.about(), t.text()))],
            )
        }
        Target::Binding(binding) => {
            let mut lines = vec![Line::from(Span::styled(binding.description, t.text()))];
            if let Some(context) = app.action_context(binding.action) {
                lines.push(Line::from(Span::styled(context, t.fg(t.accent2))));
            }
            if binding.is_global() {
                lines.push(Line::from(Span::styled("Works in any tab.", t.muted())));
            }
            (binding.label.to_owned(), lines)
        }
    };
    lines.push(Line::from(Span::styled(
        "click to run · ? for all keys",
        t.muted(),
    )));
    let area = frame.area();
    let inner = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(title.width() + 2) as u16;
    let width = (inner + 4).min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let y = if spot.area.y > area.height / 2 {
        spot.area.y.saturating_sub(height)
    } else {
        spot.area.bottom()
    };
    let x = spot.area.x.min(area.right().saturating_sub(width));
    let popup = Rect::new(x, y, width, height).intersection(area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(t.fg(t.accent))
                .padding(ratatui::widgets::Padding::horizontal(1))
                .title(Span::styled(format!(" {title} "), t.bold(t.accent))),
        ),
        popup,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [vertical] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [rect] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(vertical);
    rect
}

fn render_review(frame: &mut Frame, app: &App, review: &Review) {
    let t = &app.theme;
    let area = frame.area();
    let width = (area.width * 7 / 10).clamp(60.min(area.width), 110);
    let height = (area.height * 7 / 10).clamp(14.min(area.height), 40);
    let popup = centered(area, width, height);
    frame.render_widget(Clear, popup);
    let title = match review.phase {
        ReviewPhase::Confirm => " Review ",
        ReviewPhase::Running { .. } => " Cleaning ",
        ReviewPhase::Done { .. } => " Done ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.fg(t.accent2))
        .title(Span::styled(title, t.bold(t.accent2)));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    match &review.phase {
        ReviewPhase::Confirm => {
            let verb = match review.mode {
                DeleteMode::Permanent => "Delete",
                DeleteMode::Trash => "Move to trash",
            };
            let mut head = vec![Line::from(vec![
                Span::styled(
                    format!(
                        "{verb} {}, freeing ",
                        crate::commands::directories(review.items.len())
                    ),
                    t.text(),
                ),
                Span::styled(fmt::bytes(review.bytes), t.bold(t.success)),
            ])];
            for (hold, n) in &review.held {
                let text = match hold {
                    Hold::Recent => format!("{n} used within the last {} days", recent_days(app)),
                    Hold::Review => {
                        format!("{n} have generic names git does not confirm as ignored")
                    }
                    Hold::Protected => format!("{n} protected (skipped)"),
                };
                head.push(Line::from(Span::styled(
                    format!("! {text}"),
                    t.fg(t.warning),
                )));
            }
            if review.outside_root > 0 {
                head.push(Line::from(Span::styled(
                    format!(
                        "! {} belong to a project above the scan root",
                        review.outside_root
                    ),
                    t.fg(t.warning),
                )));
            }
            let head_height = head.len() as u16 + 1;
            let [head_area, list_area, foot_area] = Layout::vertical([
                Constraint::Length(head_height),
                Constraint::Min(1),
                Constraint::Length(3),
            ])
            .areas(inner);
            frame.render_widget(Paragraph::new(head), head_area);

            let visible = usize::from(list_area.height);
            let start = review
                .scroll
                .min(review.items.len().saturating_sub(visible));
            let list: Vec<Line> = review
                .items
                .iter()
                .skip(start)
                .take(visible)
                .filter_map(|id| app.store.entry(*id))
                .map(|entry| {
                    let bytes = entry.size().map_or(0, |s| s.reclaimable);
                    Line::from(vec![
                        Span::styled(format!("{:>9}  ", fmt::bytes(bytes)), t.size(bytes)),
                        Span::styled(
                            app.store.relative(&entry.artifact.path).into_owned(),
                            t.text(),
                        ),
                    ])
                })
                .collect();
            frame.render_widget(Paragraph::new(list), list_area);

            let mode_line = match review.mode {
                DeleteMode::Permanent => Line::from(vec![
                    Span::styled("mode ", t.muted()),
                    Span::styled("delete permanently", t.bold(t.error)),
                    Span::styled("   t: move to trash instead", t.muted()),
                ]),
                DeleteMode::Trash => Line::from(vec![
                    Span::styled("mode ", t.muted()),
                    Span::styled("move to trash", t.bold(t.warning)),
                    Span::styled(
                        " (frees nothing until emptied)   t: delete permanently",
                        t.muted(),
                    ),
                ]),
            };
            let keys = Line::from(vec![
                Span::styled("Enter", t.bold(t.success)),
                Span::styled(" confirm   ", t.muted()),
                Span::styled("Esc", t.bold(t.accent)),
                Span::styled(" cancel   ", t.muted()),
                Span::styled("↑↓", t.bold(t.accent)),
                Span::styled(" scroll", t.muted()),
            ]);
            frame.render_widget(
                Paragraph::new(vec![Line::default(), mode_line, keys]),
                foot_area,
            );
        }
        ReviewPhase::Running {
            done_bytes,
            done,
            failed,
            current,
        } => {
            let [gauge_area, info] =
                Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(inner);
            let ratio = if review.bytes == 0 {
                0.0
            } else {
                (*done_bytes as f64 / review.bytes as f64).clamp(0.0, 1.0)
            };
            frame.render_widget(
                Gauge::default()
                    .ratio(ratio)
                    .label(format!(
                        "{} / {}",
                        fmt::bytes(*done_bytes),
                        fmt::bytes(review.bytes)
                    ))
                    .gauge_style(t.fg(t.success))
                    .block(Block::default().borders(Borders::NONE)),
                gauge_area,
            );
            let mut lines = vec![Line::from(Span::styled(
                format!("{done} of {} done · {failed} failed", review.items.len()),
                t.text(),
            ))];
            if let Some(path) = current {
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", spinner(app)), t.fg(t.accent)),
                    Span::styled(app.store.relative(path).into_owned(), t.subtle()),
                ]));
            }
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("Esc stop", t.muted())));
            frame.render_widget(Paragraph::new(lines), info);
        }
        ReviewPhase::Done { report, failures } => {
            let mut lines = vec![Line::from(vec![
                Span::styled("Freed ", t.text()),
                Span::styled(fmt::bytes(report.bytes), t.bold(t.success)),
                Span::styled(
                    format!(" from {}.", crate::commands::directories(report.removed)),
                    t.text(),
                ),
            ])];
            if let (Some(before), Some(after)) = (report.free_before, report.free_after) {
                lines.push(Line::from(Span::styled(
                    format!("Free space {} → {}", fmt::bytes(before), fmt::bytes(after)),
                    t.subtle(),
                )));
            }
            if review.mode == DeleteMode::Trash {
                lines.push(Line::from(Span::styled(
                    "Moved to the trash: empty it to actually free the space.",
                    t.fg(t.warning),
                )));
            }
            if !failures.is_empty() {
                lines.push(Line::default());
                lines.push(Line::from(Span::styled(
                    format!("{} failed:", failures.len()),
                    t.bold(t.error),
                )));
                for (path, error) in failures.iter().take(12) {
                    lines.push(Line::from(vec![
                        Span::styled(app.store.relative(path).into_owned(), t.text()),
                        Span::styled(format!("  {error}"), t.fg(t.error)),
                    ]));
                }
            }
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("Press any key", t.muted())));
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
    }
}

fn recent_days(app: &App) -> u64 {
    app.policy.recent.map_or(0, |d| d.as_secs() / 86_400)
}

fn render_help(frame: &mut Frame, app: &App) {
    let t = &app.theme;
    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.fg(t.accent))
        .title(Span::styled(
            format!(" Help · {} ", app.tab.title()),
            t.bold(t.accent),
        ));
    let header = vec![
        Line::from(Span::styled(app.tab.about(), t.text())),
        Line::default(),
    ];
    let closing = Line::from(Span::styled(
        "Any key closes · hover a key in the footer for details, click it to run",
        t.muted(),
    ));

    // This tab's keys, then the keys that work everywhere (and the legend).
    let mut sections: Vec<Section> = Vec::new();
    for binding in keymap::help(app.tab) {
        if !sections.contains(&binding.section) {
            sections.push(binding.section);
        }
    }
    let mut local: Vec<Vec<Line>> = Vec::new();
    let mut global: Vec<Line> = Vec::new();
    for section in sections {
        let mut lines = vec![Line::from(Span::styled(section.title(), t.bold(t.accent2)))];
        for binding in keymap::help(app.tab).filter(|b| b.section == section) {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<10}", binding.label), t.bold(t.text)),
                Span::styled(binding.description, t.subtle()),
            ]));
        }
        lines.push(Line::default());
        if section == Section::General {
            global.extend(lines);
        } else {
            local.push(lines);
        }
    }
    if app.tab == Tab::Reclaim {
        let g = &app.glyphs;
        global.extend([
            Line::from(Span::styled("Legend", t.bold(t.accent2))),
            Line::from(vec![
                Span::styled(format!("  {} ", g.marked), t.fg(t.accent2)),
                Span::styled("marked  ", t.subtle()),
                Span::styled(format!("{} ", g.partial), t.fg(t.accent2)),
                Span::styled("partly  ", t.subtle()),
                Span::styled(format!("{} ", g.unmarked), t.muted()),
                Span::styled("unmarked", t.subtle()),
            ]),
            Line::from(vec![
                Span::styled(format!("  {} ", g.locked), t.fg(t.error)),
                Span::styled("protected: git tracks files inside", t.subtle()),
            ]),
            Line::from(vec![
                Span::styled("  ready", t.fg(t.success)),
                Span::styled(" safe to clean   ", t.subtle()),
                Span::styled("recent", t.fg(t.info)),
                Span::styled(" used lately", t.subtle()),
            ]),
            Line::from(vec![
                Span::styled("  review", t.fg(t.warning)),
                Span::styled(" generic name, not confirmed by git", t.subtle()),
            ]),
            Line::from(Span::styled("  ~ size from cache, refreshing", t.subtle())),
            Line::from(Span::styled(
                "  FREED counts only bytes deletion frees",
                t.subtle(),
            )),
            Line::default(),
        ]);
    }

    let width_of = |lines: &[Line]| lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let chrome = 4; // borders and padding
    let local_height: usize = local.iter().map(Vec::len).sum();
    let single_height = (header.len() + local_height + global.len() + 1) as u16 + 2;
    let block = block.padding(ratatui::widgets::Padding::horizontal(1));
    if single_height <= area.height {
        let mut lines = header;
        lines.extend(local.into_iter().flatten());
        lines.extend(global);
        lines.push(closing);
        let popup = centered(area, width_of(&lines) + chrome, single_height);
        frame.render_widget(Clear, popup);
        frame.render_widget(Paragraph::new(lines).block(block), popup);
        return;
    }
    // Too tall for the terminal: this tab's keys beside the global ones. Trailing tab
    // sections move to the right column while that makes the popup shorter.
    let mut right_sections: Vec<Vec<Line>> = Vec::new();
    let (mut left_height, mut right_height) = (local_height, global.len());
    while local.len() > 1 {
        let moved = local.last().map_or(0, Vec::len);
        if left_height.max(right_height) <= (left_height - moved).max(right_height + moved) {
            break;
        }
        right_sections.insert(0, local.pop().unwrap_or_default());
        (left_height, right_height) = (left_height - moved, right_height + moved);
    }
    let local: Vec<Line> = local.into_iter().flatten().collect();
    let global: Vec<Line> = right_sections.into_iter().flatten().chain(global).collect();
    const GUTTER: u16 = 3;
    let (left_width, right_width) = (width_of(&local), width_of(&global));
    let body = local.len().max(global.len()) as u16;
    let width = (left_width + GUTTER + right_width).max(width_of(&header)) + chrome;
    let popup = centered(area, width, header.len() as u16 + body + 1 + 2);
    frame.render_widget(Clear, popup);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let [top, columns, bottom] = Layout::vertical([
        Constraint::Length(header.len() as u16),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let [left, _, right] = Layout::horizontal([
        Constraint::Length(left_width),
        Constraint::Length(GUTTER),
        Constraint::Fill(1),
    ])
    .areas(columns);
    frame.render_widget(Paragraph::new(header).wrap(Wrap { trim: true }), top);
    frame.render_widget(Paragraph::new(local), left);
    frame.render_widget(Paragraph::new(global), right);
    frame.render_widget(Paragraph::new(closing), bottom);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::tests::app;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn draw(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    fn press(app: &mut App, code: KeyCode) {
        app.update(crate::tui::app::Msg::Key(KeyEvent::new(
            code,
            KeyModifiers::NONE,
        )));
    }

    #[test]
    fn renders_at_every_size_without_panicking() {
        for mode in ['\0', '?', 'd', '/'] {
            let mut app = app();
            press(&mut app, KeyCode::Char('a'));
            if mode != '\0' {
                press(&mut app, KeyCode::Char(mode));
            }
            for (w, h) in [(1, 1), (10, 3), (40, 8), (80, 24), (120, 40), (200, 60)] {
                draw(&mut app, w, h);
            }
        }
    }

    fn mouse(app: &mut App, kind: ratatui::crossterm::event::MouseEventKind, area: Rect) {
        app.update(crate::tui::app::Msg::Mouse(
            ratatui::crossterm::event::MouseEvent {
                kind,
                column: area.x,
                row: area.y,
                modifiers: KeyModifiers::NONE,
            },
        ));
    }

    #[test]
    fn footer_separates_tab_keys_from_global_ones() {
        let mut app = app();
        let screen = draw(&mut app, 140, 20);
        let footer = screen.lines().last().unwrap();
        let global = footer.find("any tab").expect(footer);
        assert!(footer.find("d clean").unwrap() < global, "{footer}");
        assert!(footer.find("? help").unwrap() > global, "{footer}");
        assert!(footer.find("q quit").unwrap() > global, "{footer}");
        // Narrow terminals keep the global keys and drop tab keys from the end.
        let screen = draw(&mut app, 60, 20);
        let footer = screen.lines().last().unwrap();
        assert!(footer.contains("q quit"), "{footer}");
    }

    #[test]
    fn hovering_explains_and_clicking_runs() {
        use crate::tui::keymap::Action;
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let mut app = app();
        draw(&mut app, 140, 20);
        let clean = app
            .hotspots
            .iter()
            .find(|h| matches!(h.target, Target::Binding(b) if b.action == Action::Clean))
            .copied()
            .unwrap();
        mouse(&mut app, MouseEventKind::Moved, clean.area);
        let screen = draw(&mut app, 140, 20);
        assert!(
            screen.contains("review and clean marked artifacts"),
            "{screen}"
        );
        assert!(screen.contains("Nothing marked yet"), "{screen}");
        // Moving within the same hotspot does not redraw (the event loop clears `dirty`
        // after drawing).
        app.dirty = false;
        mouse(&mut app, MouseEventKind::Moved, clean.area);
        assert!(!app.dirty);
        // ...but it does not cancel a redraw an earlier message of the batch asked for.
        press(&mut app, KeyCode::Char('j'));
        mouse(&mut app, MouseEventKind::Moved, clean.area);
        assert!(app.dirty);

        let caches = app
            .hotspots
            .iter()
            .find(|h| matches!(h.target, Target::Tab(Tab::Caches)))
            .copied()
            .unwrap();
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            caches.area,
        );
        assert_eq!(app.tab, Tab::Caches);
        assert!(app.hover.is_none());
    }

    #[test]
    fn help_fits_short_terminals_in_two_columns() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        let screen = draw(&mut app, 160, 30);
        assert!(screen.contains("Help · Reclaim"), "{screen}");
        assert!(screen.contains("Any tab"), "{screen}");
        assert!(screen.contains("Any key closes"), "{screen}");
    }

    #[test]
    fn main_screen_shows_rows_totals_and_details() {
        let mut app = app();
        press(&mut app, KeyCode::Char('E'));
        let screen = draw(&mut app, 140, 20);
        assert!(screen.contains("carwash"));
        assert!(screen.contains("target"));
        assert!(screen.contains("protected"));
        assert!(screen.contains("recent"));
        assert!(screen.contains("1.00 kB reclaimable"), "{screen}");
        assert!(screen.contains("Details"));
    }

    #[test]
    fn review_lists_marked_items() {
        let mut app = app();
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('d'));
        let screen = draw(&mut app, 120, 30);
        assert!(
            screen.contains("Delete 1 directory, freeing 500 B"),
            "{screen}"
        );
        assert!(screen.contains("a/target"));
        assert!(screen.contains("delete permanently"));
    }
}
