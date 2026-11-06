//! Full-screen view for exploring Claude Code memories, execution history, and past conversations.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::storage::{
    ConversationRecord, Db, ExecutionRecord, MemoryRecord, SessionPacket, TimelineItem, Timezone,
};
use crate::ui::style;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryTab {
    Memories,
    Timeline,
    Executions,
    Conversations,
}

impl MemoryTab {
    pub const ALL: [MemoryTab; 4] = [
        MemoryTab::Memories,
        MemoryTab::Timeline,
        MemoryTab::Executions,
        MemoryTab::Conversations,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            MemoryTab::Memories => "1. 메모리",
            MemoryTab::Timeline => "2. 타임라인 / 관찰",
            MemoryTab::Executions => "3. 실행 이력",
            MemoryTab::Conversations => "4. 대화 기록",
        }
    }

    pub fn short_label(&self) -> &'static str {
        match self {
            MemoryTab::Memories => "1.메모리",
            MemoryTab::Timeline => "2.타임라인",
            MemoryTab::Executions => "3.실행",
            MemoryTab::Conversations => "4.대화",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            MemoryTab::Memories => MemoryTab::Timeline,
            MemoryTab::Timeline => MemoryTab::Executions,
            MemoryTab::Executions => MemoryTab::Conversations,
            MemoryTab::Conversations => MemoryTab::Memories,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            MemoryTab::Memories => MemoryTab::Conversations,
            MemoryTab::Timeline => MemoryTab::Memories,
            MemoryTab::Executions => MemoryTab::Timeline,
            MemoryTab::Conversations => MemoryTab::Executions,
        }
    }
}

pub struct MemoryView {
    pub tab: MemoryTab,
    pub cursor: usize,
    pub detail_scroll: usize,
    pub search_query: String,
    pub is_searching: bool,
    pub memories: Vec<MemoryRecord>,
    pub timeline: Vec<TimelineItem>,
    pub executions: Vec<ExecutionRecord>,
    pub conversations: Vec<ConversationRecord>,
    pub session_packet: Option<SessionPacket>,
    pub session_packet_scroll: usize,
    #[allow(dead_code)]
    pub status_message: Option<String>,
}

impl Default for MemoryView {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryView {
    pub fn new() -> Self {
        Self {
            tab: MemoryTab::Memories,
            cursor: 0,
            detail_scroll: 0,
            search_query: String::new(),
            is_searching: false,
            memories: Vec::new(),
            timeline: Vec::new(),
            executions: Vec::new(),
            conversations: Vec::new(),
            session_packet: None,
            session_packet_scroll: 0,
            status_message: None,
        }
    }

    /// Reloads data from the SQLite database.
    pub fn refresh(&mut self, db: &Db) {
        let q = if self.search_query.is_empty() {
            None
        } else {
            Some(self.search_query.as_str())
        };

        if let Ok(m) = db.list_memories(None, q, 100) {
            self.memories = m;
        }
        if let Ok(t) = db.get_timeline(None, None, 100) {
            if let Some(query) = q {
                self.timeline = t
                    .into_iter()
                    .filter(|x| x.title.contains(query) || x.detail.contains(query))
                    .collect();
            } else {
                self.timeline = t;
            }
        }
        if let Ok(e) = db.list_executions(None, None, None, 100) {
            if let Some(query) = q {
                self.executions = e
                    .into_iter()
                    .filter(|x| x.command_hint.contains(query) || x.input_json.contains(query))
                    .collect();
            } else {
                self.executions = e;
            }
        }
        if let Ok(c) = db.list_conversations(None, None, q, 100) {
            self.conversations = c;
        }

        self.clamp_cursor();
    }

    pub fn clamp_cursor(&mut self) {
        let len = self.current_len();
        if len == 0 {
            self.cursor = 0;
        } else if self.cursor >= len {
            self.cursor = len - 1;
        }
    }

    pub fn current_len(&self) -> usize {
        match self.tab {
            MemoryTab::Memories => self.memories.len(),
            MemoryTab::Timeline => self.timeline.len(),
            MemoryTab::Executions => self.executions.len(),
            MemoryTab::Conversations => self.conversations.len(),
        }
    }

    pub fn move_cursor_down(&mut self) {
        let len = self.current_len();
        if len > 0 && self.cursor + 1 < len {
            self.cursor += 1;
            self.detail_scroll = 0;
        }
    }

    pub fn move_cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.detail_scroll = 0;
        }
    }

    pub fn scroll_detail_down(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_add(3);
    }

    pub fn scroll_detail_up(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_sub(3);
    }

    pub fn switch_tab(&mut self, next: bool) {
        self.tab = if next { self.tab.next() } else { self.tab.prev() };
        self.cursor = 0;
        self.detail_scroll = 0;
    }

    pub fn open_session(&mut self, session_id: &str, db: &Db) {
        if let Ok(Some(pkt)) = db.get_session_packet(session_id) {
            self.session_packet = Some(pkt);
            self.session_packet_scroll = 0;
        } else {
            let _ = crate::storage::sync_once(db, &crate::storage::default_projects_dir());
            if let Ok(Some(pkt)) = db.get_session_packet(session_id) {
                self.session_packet = Some(pkt);
                self.session_packet_scroll = 0;
            } else {
                self.search_query = session_id.chars().take(8).collect();
                self.refresh(db);
            }
        }
    }

    pub fn open_selected_session_packet(&mut self, db: &Db) {
        let sid = match self.tab {
            MemoryTab::Memories => self.memories.get(self.cursor).and_then(|m| m.origin_session_id.clone()),
            MemoryTab::Timeline => self.timeline.get(self.cursor).map(|t| t.session_id.clone()),
            MemoryTab::Executions => self.executions.get(self.cursor).map(|e| e.session_id.clone()),
            MemoryTab::Conversations => self.conversations.get(self.cursor).map(|c| c.session_id.clone()),
        };

        if let Some(s) = sid.filter(|s| !s.is_empty()) {
            self.open_session(&s, db);
        }
    }

    pub fn scroll_packet_down(&mut self, delta: usize) {
        self.session_packet_scroll = self.session_packet_scroll.saturating_add(delta);
    }

    pub fn scroll_packet_up(&mut self, delta: usize) {
        self.session_packet_scroll = self.session_packet_scroll.saturating_sub(delta);
    }
}

/// Renders the memory & history explorer modal view.
pub fn render_memory_view(f: &mut Frame, area: Rect, view: &MemoryView, tick: u64, tz: Timezone) {
    if area.width < 30 || area.height < 10 {
        return;
    }

    f.render_widget(Clear, area);

    // Frame with game-style border
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(style::BORDER)
        .border_style(Style::default().fg(style::SELECTED))
        .title(Span::styled(" [ Claude Code 과거 메모리 & 히스토리 보관소 ] ", Style::default().fg(style::TEXT).add_modifier(Modifier::BOLD)));
    f.render_widget(block, area);

    let inner = area.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 1,
    });

    let [header_area, content_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    if let Some(ref packet) = view.session_packet {
        render_session_packet_overlay(f, content_area, packet, view.session_packet_scroll, tz);
        let help = " Esc/q/Enter: 닫기 · j/k: 이동 · d/u: 10줄 스크롤 ";
        f.render_widget(
            Paragraph::new(Line::styled(help, Style::default().fg(style::SELECTED).add_modifier(Modifier::BOLD)))
                .alignment(Alignment::Center),
            footer_area,
        );
        return;
    }

    // Render Tab bar & search bar
    render_header(f, header_area, view);

    // Render Split Pane: List (38%) + Details (62%)
    let [list_area, detail_area] = Layout::horizontal([
        Constraint::Percentage(38),
        Constraint::Percentage(62),
    ])
    .areas(content_area);

    render_list(f, list_area, view, tick, tz);
    render_detail(f, detail_area, view, tz);

    // Render Footer
    render_footer(f, footer_area, view);
}

fn render_header(f: &mut Frame, area: Rect, view: &MemoryView) {
    let search_len = if area.width < 90 { 20 } else { 32 };
    let [tabs_rect, search_rect] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(search_len),
    ])
    .areas(area);

    let is_compact = area.width < 90;
    let mut tab_spans = Vec::new();
    for tab in MemoryTab::ALL {
        let is_sel = tab == view.tab;
        let style = if is_sel {
            Style::default().fg(style::HUD_BADGE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(style::TEXT_DIM)
        };
        let prefix = if is_sel { "▶ " } else { "  " };
        let label = if is_compact { tab.short_label() } else { tab.label() };
        tab_spans.push(Span::styled(format!("{prefix}{label}"), style));
        let sep = if is_compact { "  " } else { "    " };
        tab_spans.push(Span::raw(sep));
    }
    f.render_widget(Paragraph::new(Line::from(tab_spans)), tabs_rect);

    // Search bar
    let search_style = if view.is_searching {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(style::TEXT_DIM)
    };
    let query_display = if view.search_query.is_empty() {
        if view.is_searching { "입력 중…" } else { "/ 키로 검색" }
    } else {
        &view.search_query
    };
    let search_text = format!("🔍 {query_display}");
    f.render_widget(
        Paragraph::new(Line::styled(search_text, search_style)).alignment(Alignment::Right),
        search_rect,
    );
}

fn render_list(f: &mut Frame, area: Rect, view: &MemoryView, tick: u64, tz: Timezone) {
    let title = match view.tab {
        MemoryTab::Memories => format!(" 메모리 목록 ({}) ", view.memories.len()),
        MemoryTab::Timeline => format!(" 타임라인 ({}) ", view.timeline.len()),
        MemoryTab::Executions => format!(" 도구 실행 ({}) ", view.executions.len()),
        MemoryTab::Conversations => format!(" 대화 목록 ({}) ", view.conversations.len()),
    };

    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(style::TEXT_DIM))
        .title(Span::styled(title, Style::default().fg(style::TEXT_DIM)));
    f.render_widget(block, area);

    let list_inner = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width.saturating_sub(1),
        height: area.height.saturating_sub(1),
    };

    if view.current_len() == 0 {
        let empty_msg = Paragraph::new(Line::styled(
            "항목이 없습니다. ('r'로 새로고침)",
            Style::default().fg(style::TEXT_DIM),
        ));
        f.render_widget(empty_msg, list_inner);
        return;
    }

    let visible_rows = list_inner.height as usize;
    let offset = if view.cursor >= visible_rows {
        view.cursor - visible_rows + 1
    } else {
        0
    };

    let cursor_active = style::cursor_visible(tick);

    let mut lines = Vec::new();
    let max_idx = (offset + visible_rows).min(view.current_len());

    for idx in offset..max_idx {
        let is_sel = idx == view.cursor;
        let mark = if is_sel {
            if cursor_active { style::CURSOR } else { " " }
        } else {
            " "
        };

        let row_line = match view.tab {
            MemoryTab::Memories => {
                let m = &view.memories[idx];
                let title = if !m.title.is_empty() { &m.title } else { &m.file_name };
                let short_title = truncate(title, (list_inner.width.saturating_sub(16)) as usize);
                let badge = format!("[{}]", truncate(&m.project_name, 10));

                let style = if is_sel {
                    Style::default().fg(style::SELECTED).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(style::TEXT)
                };

                Line::from(vec![
                    Span::styled(format!("{mark} "), Style::default().fg(style::SELECTED)),
                    Span::styled(badge, Style::default().fg(style::TEXT_DIM)),
                    Span::raw(" "),
                    Span::styled(short_title, style),
                ])
            }
            MemoryTab::Timeline => {
                let item = &view.timeline[idx];
                let time_str = tz.format_epoch_time(item.timestamp_ms);
                let short_title = truncate(&item.title.replace('\n', " "), (list_inner.width.saturating_sub(28)) as usize);
                let badge = format!("[{}]", truncate(&item.project, 8));
                let type_color = match item.badge.as_str() {
                    "DECISION" => Color::Magenta,
                    "CHANGE" => Color::Green,
                    "BUGFIX" => Color::Red,
                    "DISCOVERY" => Color::Cyan,
                    "MEMORY" => Color::Yellow,
                    _ => style::HUD_BADGE,
                };

                Line::from(vec![
                    Span::styled(format!("{mark} "), Style::default().fg(style::SELECTED)),
                    Span::styled(format!("{time_str} "), Style::default().fg(style::TEXT_DIM)),
                    Span::styled(format!("{:<8} ", format!("[{}]", item.badge)), Style::default().fg(type_color).add_modifier(Modifier::BOLD)),
                    Span::styled(badge, Style::default().fg(style::TEXT_DIM)),
                    Span::raw(" "),
                    Span::styled(short_title, if is_sel { Style::default().fg(style::SELECTED).add_modifier(Modifier::BOLD) } else { Style::default().fg(style::TEXT) }),
                ])
            }
            MemoryTab::Executions => {
                let e = &view.executions[idx];
                let time_str = tz.format_epoch_time(e.timestamp_ms);
                let icon = match e.status.as_str() {
                    "success" => "✓",
                    "error" => "✗",
                    _ => "⟳",
                };
                let icon_color = match e.status.as_str() {
                    "success" => style::DONE,
                    "error" => style::FLASH_ALERT,
                    _ => style::RUNNING,
                };
                let short_hint = truncate(&e.command_hint.replace('\n', " "), (list_inner.width.saturating_sub(26)) as usize);

                Line::from(vec![
                    Span::styled(format!("{mark} "), Style::default().fg(style::SELECTED)),
                    Span::styled(format!("{time_str} "), Style::default().fg(style::TEXT_DIM)),
                    Span::styled(format!("{icon} "), Style::default().fg(icon_color)),
                    Span::styled(format!("{:<6} ", truncate(&e.tool_name, 6)), Style::default().fg(style::HUD_BADGE)),
                    Span::styled(short_hint, if is_sel { Style::default().fg(style::SELECTED).add_modifier(Modifier::BOLD) } else { Style::default().fg(style::TEXT) }),
                ])
            }
            MemoryTab::Conversations => {
                let c = &view.conversations[idx];
                let time_str = tz.format_epoch_time(c.timestamp_ms);
                let role_icon = if c.role == "user" { "👤" } else { "🤖" };
                let short_content = truncate(&c.content.replace('\n', " "), (list_inner.width.saturating_sub(22)) as usize);

                Line::from(vec![
                    Span::styled(format!("{mark} "), Style::default().fg(style::SELECTED)),
                    Span::styled(format!("{time_str} "), Style::default().fg(style::TEXT_DIM)),
                    Span::raw(format!("{role_icon} ")),
                    Span::styled(short_content, if is_sel { Style::default().fg(style::SELECTED).add_modifier(Modifier::BOLD) } else { Style::default().fg(style::TEXT) }),
                ])
            }
        };

        lines.push(row_line);
    }

    f.render_widget(Paragraph::new(lines), list_inner);
}

fn render_detail(f: &mut Frame, area: Rect, view: &MemoryView, tz: Timezone) {
    let block = Block::default()
        .borders(Borders::NONE)
        .title(Span::styled(" 상세 내용 ", Style::default().fg(style::TEXT_DIM)));
    f.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(1),
    };

    if view.current_len() == 0 {
        return;
    }

    let mut lines = Vec::new();

    match view.tab {
        MemoryTab::Memories => {
            if let Some(m) = view.memories.get(view.cursor) {
                lines.push(Line::styled(format!("제목: {}", m.title), Style::default().fg(style::HUD_BADGE).add_modifier(Modifier::BOLD)));
                lines.push(Line::styled(format!("프로젝트: {}  |  파일명: {}", m.project_name, m.file_name), Style::default().fg(style::TEXT_DIM)));
                if let Some(origin) = &m.origin_session_id {
                    lines.push(Line::styled(format!("출처 세션: {}", origin), Style::default().fg(style::TEXT_DIM)));
                }
                if !m.description.is_empty() {
                    lines.push(Line::styled(format!("설명: {}", m.description), Style::default().fg(Color::Cyan)));
                }
                lines.push(Line::styled(format!("최종 수정: {} ({})", tz.format_iso_utc(&m.updated_at), tz.as_code()), Style::default().fg(style::TEXT_DIM)));
                lines.push(Line::raw("─".repeat(inner.width as usize)));

                for line in m.content.lines() {
                    let style = if line.starts_with('#') {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else if line.starts_with('-') || line.starts_with('*') {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(style::TEXT)
                    };
                    lines.push(Line::styled(line, style));
                }
            }
        }
        MemoryTab::Timeline => {
            if let Some(item) = view.timeline.get(view.cursor) {
                lines.push(Line::styled(format!("제목: {}", item.title), Style::default().fg(style::HUD_BADGE).add_modifier(Modifier::BOLD)));
                lines.push(Line::styled(format!("구분: [{}]  |  프로젝트: {}", item.badge, item.project), Style::default().fg(style::TEXT_DIM)));
                lines.push(Line::styled(format!("발생 시각: {} ({})", tz.format_epoch_ms(item.timestamp_ms), tz.as_code()), Style::default().fg(style::TEXT_DIM)));
                if !item.session_id.is_empty() {
                    lines.push(Line::styled(format!("세션 ID: {}", item.session_id), Style::default().fg(style::TEXT_DIM)));
                }
                lines.push(Line::raw("─".repeat(inner.width as usize)));

                lines.push(Line::styled("[상세 내용 / Narrative]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
                for line in item.detail.lines() {
                    lines.push(Line::styled(line, Style::default().fg(style::TEXT)));
                }
            }
        }
        MemoryTab::Executions => {
            if let Some(e) = view.executions.get(view.cursor) {
                lines.push(Line::styled(format!("도구: {}", e.tool_name), Style::default().fg(style::HUD_BADGE).add_modifier(Modifier::BOLD)));
                lines.push(Line::styled(format!("상태: {}  |  프로젝트: {}", e.status, e.project_name), Style::default().fg(style::TEXT_DIM)));
                lines.push(Line::styled(format!("실행 시각: {} ({})", tz.format_epoch_ms(e.timestamp_ms), tz.as_code()), Style::default().fg(style::TEXT_DIM)));
                lines.push(Line::styled(format!("세션 ID: {}", e.session_id), Style::default().fg(style::TEXT_DIM)));
                lines.push(Line::styled(format!("도구 호출 ID: {}", e.tool_use_id), Style::default().fg(style::TEXT_DIM)));
                lines.push(Line::raw("─".repeat(inner.width as usize)));

                lines.push(Line::styled("[명령어 / 인자]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
                for line in e.input_json.lines() {
                    lines.push(Line::styled(line, Style::default().fg(style::TEXT)));
                }

                if !e.result_snippet.is_empty() {
                    lines.push(Line::raw(""));
                    lines.push(Line::styled("[실행 결과 요약]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)));
                    for line in e.result_snippet.lines() {
                        lines.push(Line::styled(line, Style::default().fg(style::TEXT)));
                    }
                }
            }
        }
        MemoryTab::Conversations => {
            if let Some(c) = view.conversations.get(view.cursor) {
                let role_label = if c.role == "user" { "사용자 (User)" } else { "Claude 어시스턴트" };
                lines.push(Line::styled(format!("발화자: {}", role_label), Style::default().fg(style::HUD_BADGE).add_modifier(Modifier::BOLD)));
                lines.push(Line::styled(format!("프로젝트: {}  |  세션 ID: {}", c.project_name, c.session_id), Style::default().fg(style::TEXT_DIM)));
                if let Some(m) = &c.model {
                    lines.push(Line::styled(format!("모델: {}", m), Style::default().fg(style::TEXT_DIM)));
                }
                lines.push(Line::styled(format!("기록 시각: {} ({})", tz.format_epoch_ms(c.timestamp_ms), tz.as_code()), Style::default().fg(style::TEXT_DIM)));
                lines.push(Line::raw("─".repeat(inner.width as usize)));

                for line in c.content.lines() {
                    lines.push(Line::styled(line, Style::default().fg(style::TEXT)));
                }
            }
        }
    }

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((view.detail_scroll as u16, 0));
    f.render_widget(paragraph, inner);
}

fn render_footer(f: &mut Frame, area: Rect, view: &MemoryView) {
    let help = if view.is_searching {
        " [검색] 글자 입력 후 Enter: 완료 · Esc: 취소 "
    } else if area.width < 90 {
        " Tab: 전환 · jk: 이동 · Enter: 세션전체 · /: 검색 · Esc: 닫기 "
    } else {
        " Tab: 전환 · jk: 이동 · du: 스크롤 · Enter: 세션전체 · /: 검색 · r: 갱신 · s: Status · Esc: 닫기 "
    };

    f.render_widget(
        Paragraph::new(Line::styled(help, Style::default().fg(style::TEXT_DIM))).alignment(Alignment::Center),
        area,
    );
}

fn render_session_packet_overlay(
    f: &mut Frame,
    area: Rect,
    packet: &SessionPacket,
    scroll: usize,
    tz: Timezone,
) {
    let title = format!(" [ 세션 통합 메모리: {} ] ", packet.session_id);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(style::BORDER)
        .border_style(Style::default().fg(style::HUD_BADGE))
        .title(Span::styled(
            title,
            Style::default().fg(style::SELECTED).add_modifier(Modifier::BOLD),
        ));

    let markdown = packet.to_markdown(tz);
    let lines: Vec<Line> = markdown
        .lines()
        .map(|l| {
            if l.starts_with("# ") {
                Line::styled(l, Style::default().fg(style::SELECTED).add_modifier(Modifier::BOLD))
            } else if l.starts_with("## ") {
                Line::styled(l, Style::default().fg(style::HUD_BADGE).add_modifier(Modifier::BOLD))
            } else if l.starts_with("### ") {
                Line::styled(l, Style::default().fg(style::NOTIFY_BLUE).add_modifier(Modifier::BOLD))
            } else if l.starts_with("- **") {
                Line::styled(l, Style::default().fg(style::TEXT))
            } else if l.starts_with("```") {
                Line::styled(l, Style::default().fg(style::TEXT_DIM))
            } else if l.starts_with("  > ") {
                Line::styled(l, Style::default().fg(style::TEXT_DIM))
            } else {
                Line::styled(l, Style::default().fg(style::TEXT))
            }
        })
        .collect();

    let p = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));

    f.render_widget(p, area);
}

fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_len {
        s.to_string()
    } else {
        let truncated: String = chars.iter().take(max_len.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_util::{draw, rows};

    #[test]
    fn test_memory_view_navigation_and_tabs() {
        let mut view = MemoryView::new();
        assert_eq!(view.tab, MemoryTab::Memories);

        view.switch_tab(true);
        assert_eq!(view.tab, MemoryTab::Timeline);

        view.switch_tab(true);
        assert_eq!(view.tab, MemoryTab::Executions);

        view.switch_tab(true);
        assert_eq!(view.tab, MemoryTab::Conversations);

        view.switch_tab(true);
        assert_eq!(view.tab, MemoryTab::Memories);

        view.switch_tab(false);
        assert_eq!(view.tab, MemoryTab::Conversations);

        // Add dummy memories
        view.tab = MemoryTab::Memories;
        view.memories.push(MemoryRecord {
            id: 1,
            project_name: "test-proj".into(),
            file_name: "test.md".into(),
            title: "Test Title".into(),
            description: "A test desc".into(),
            memory_type: "project".into(),
            origin_session_id: None,
            content: "Some content".into(),
            mtime_ms: 100,
            updated_at: "2026-11-06 10:00:00".into(),
        });
        view.memories.push(MemoryRecord {
            id: 2,
            project_name: "test-proj".into(),
            file_name: "test2.md".into(),
            title: "Test Title 2".into(),
            description: "Desc 2".into(),
            memory_type: "project".into(),
            origin_session_id: None,
            content: "Content 2".into(),
            mtime_ms: 200,
            updated_at: "2026-11-06 10:05:00".into(),
        });

        assert_eq!(view.current_len(), 2);
        assert_eq!(view.cursor, 0);

        view.move_cursor_down();
        assert_eq!(view.cursor, 1);

        view.move_cursor_down(); // clamp
        assert_eq!(view.cursor, 1);

        view.move_cursor_up();
        assert_eq!(view.cursor, 0);

        view.scroll_detail_down();
        assert_eq!(view.detail_scroll, 3);
        view.scroll_detail_up();
        assert_eq!(view.detail_scroll, 0);
    }

    #[test]
    fn test_render_memory_view_smoke() {
        let mut view = MemoryView::new();
        view.memories.push(MemoryRecord {
            id: 1,
            project_name: "test-proj".into(),
            file_name: "test.md".into(),
            title: "Test Title".into(),
            description: "A test desc".into(),
            memory_type: "project".into(),
            origin_session_id: None,
            content: "# Header\nLine 1\nLine 2".into(),
            mtime_ms: 100,
            updated_at: "2026-11-06 10:00:00".into(),
        });

        let buf = draw(100, 30, |f| {
            render_memory_view(f, f.area(), &view, 0, Timezone::Kst);
        });
        let screen = rows(&buf).join("\n");
        assert!(screen.contains("Claude Code 과거 메모리 & 히스토리 보관소"));
        assert!(screen.contains("Test Title"));
        assert!(screen.contains("test-proj"));
    }

    #[test]
    fn test_render_session_packet_overlay() {
        let mut view = MemoryView::new();
        view.session_packet = Some(SessionPacket {
            session_id: "aaaaaaaa-0000-0000-0000-000000000001".into(),
            project_name: "test-proj".into(),
            started_at_ms: Some(1_700_000_000_000),
            ended_at_ms: Some(1_700_000_060_000),
            stats: Default::default(),
            observations: Vec::new(),
            conversations: Vec::new(),
            executions: Vec::new(),
            memories: Vec::new(),
        });

        let buf = draw(100, 30, |f| {
            render_memory_view(f, f.area(), &view, 0, Timezone::Kst);
        });
        let screen = rows(&buf).join("\n");
        assert!(screen.contains("세션 통합 메모리: aaaaaaaa-0000-0000-0000-000000000001"));
        assert!(screen.contains("Session Memory Report"));
        assert!(screen.contains("Esc/q/Enter: 닫기"));
    }
}
