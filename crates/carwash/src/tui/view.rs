//! Rendering. Pure functions of [`App`], except for recording table geometry for the mouse.

use super::app::{App, Level, Mode, Review, ReviewPhase};
use super::keymap::{self, Section, Tab};
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
    render_footer(frame, app, footer);

    match &app.mode {
        Mode::Review(review) => render_review(frame, app, review),
        Mode::Help => render_help(frame, app),
        _ => {}
    }
}

fn home_relative(path: &std::path::Path) -> String {
    if let Some(home) = std::env::var_os("HOME")
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

fn render_toolbar(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let mut left = vec![Span::raw(" ")];
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let style = if *tab == app.tab {
            t.bold(t.accent).add_modifier(Modifier::REVERSED)
        } else {
            t.muted()
        };
        left.push(Span::styled(format!(" {} {} ", i + 1, tab.title()), style));
        left.push(Span::raw(" "));
    }
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
    let marked_bytes: u64 = app
        .marked
        .iter()
        .filter_map(|id| app.store.entry(*id))
        .filter_map(|e| e.size())
        .map(|s| s.reclaimable)
        .sum();
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

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    if matches!(app.mode, Mode::Search) {
        let prompt = "/ ";
        let width = area.width.saturating_sub(prompt.len() as u16 + 1) as usize;
        let scroll = app.input.visual_scroll(width);
        let line = Line::from(vec![
            Span::styled(prompt, t.bold(t.accent)),
            Span::styled(
                app.input.value().chars().skip(scroll).collect::<String>(),
                t.text(),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        let cursor = (app.input.visual_cursor().saturating_sub(scroll)) as u16;
        frame.set_cursor_position((area.x + prompt.len() as u16 + cursor, area.y));
        return;
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
        return;
    }
    let hints: &[(&str, &str)] = match app.tab {
        Tab::Reclaim => &[
            ("space", "mark"),
            ("a", "mark ready"),
            ("d", "clean"),
            ("/", "filter"),
            ("tab", "group"),
            ("s", "sort"),
            ("2", "tasks"),
            ("?", "help"),
            ("q", "quit"),
        ],
        Tab::Tasks => &[
            ("enter", "run"),
            ("space", "mark project"),
            ("tab", "pane"),
            ("x", "stop"),
            ("[ ]", "jobs"),
            ("/", "filter"),
            ("1", "reclaim"),
            ("?", "help"),
            ("q", "quit"),
        ],
        Tab::Updates => &[
            ("enter", "check"),
            ("C", "check all"),
            ("space", "mark"),
            ("u", "update"),
            ("U", "upgrade"),
            ("tab", "pane"),
            ("/", "filter"),
            ("?", "help"),
            ("q", "quit"),
        ],
        Tab::Caches => &[
            ("space", "mark"),
            ("d", "clean"),
            ("r", "measure"),
            ("o", "reveal"),
            ("1", "reclaim"),
            ("?", "help"),
            ("q", "quit"),
        ],
    };
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", t.muted()));
        }
        spans.push(Span::styled(*key, t.bold(t.accent)));
        spans.push(Span::styled(format!(" {label}"), t.muted()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
    let popup = centered(area, 84, 40);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(t.fg(t.accent))
        .title(Span::styled(" Help ", t.bold(t.accent)));
    let mut lines: Vec<Line> = Vec::new();
    let mut sections: Vec<Section> = Vec::new();
    for binding in keymap::help(app.tab) {
        if !sections.contains(&binding.section) {
            sections.push(binding.section);
        }
    }
    for section in sections {
        lines.push(Line::from(Span::styled(section.title(), t.bold(t.accent2))));
        for binding in keymap::help(app.tab).filter(|b| b.section == section) {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<10}", binding.label), t.bold(t.text)),
                Span::styled(binding.description, t.subtle()),
            ]));
        }
        lines.push(Line::default());
    }
    if app.tab != Tab::Reclaim {
        lines.push(Line::from(Span::styled(
            "Press any key to close",
            t.muted(),
        )));
        frame.render_widget(Paragraph::new(lines).block(block), popup);
        return;
    }
    let g = &app.glyphs;
    lines.push(Line::from(Span::styled("Legend", t.bold(t.accent2))));
    lines.push(Line::from(vec![
        Span::styled(format!("  {} ", g.marked), t.fg(t.accent2)),
        Span::styled("marked  ", t.subtle()),
        Span::styled(format!("{} ", g.partial), t.fg(t.accent2)),
        Span::styled("partly  ", t.subtle()),
        Span::styled(format!("{} ", g.unmarked), t.muted()),
        Span::styled("unmarked  ", t.subtle()),
        Span::styled(format!("{} ", g.locked), t.fg(t.error)),
        Span::styled("protected", t.subtle()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  ready", t.fg(t.success)),
        Span::styled(" safe to clean  ", t.subtle()),
        Span::styled("recent", t.fg(t.info)),
        Span::styled(" used lately  ", t.subtle()),
        Span::styled("review", t.fg(t.warning)),
        Span::styled(" generic name  ", t.subtle()),
        Span::styled("protected", t.fg(t.error)),
        Span::styled(" git-tracked", t.subtle()),
    ]));
    lines.push(Line::from(Span::styled(
        "  ~ size from cache, refreshing   FREED counts only bytes deletion frees",
        t.subtle(),
    )));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Press any key to close",
        t.muted(),
    )));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
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
