use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub type Logs = Arc<Mutex<VecDeque<String>>>;
#[derive(Clone)]
pub struct LogSink {
    pub logs: Logs,
    file: Arc<Mutex<File>>,
    interactive: bool,
}
impl LogSink {
    pub fn new(file: File, interactive: bool) -> Self {
        Self {
            logs: Arc::default(),
            file: Arc::new(Mutex::new(file)),
            interactive,
        }
    }
}
pub struct LogWriter {
    sink: LogSink,
    pending: Vec<u8>,
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LogWriter {
            sink: self.clone(),
            pending: Vec::new(),
        }
    }
}
impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Drop for LogWriter {
    fn drop(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        if let Ok(mut file) = self.sink.file.lock() {
            let _ = file.write_all(&self.pending);
            let _ = file.flush();
        }
        if !self.sink.interactive {
            let _ = io::stdout().write_all(&self.pending);
        }
        let text = String::from_utf8_lossy(&self.pending);
        if let Ok(mut logs) = self.sink.logs.lock() {
            for line in text.lines() {
                logs.push_back(line.to_owned());
            }
            while logs.len() > 1000 {
                logs.pop_front();
            }
        }
    }
}

struct RestoreTerminal;
impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            event::DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}

#[derive(Default)]
struct Ui {
    selected: usize,
    offset: usize,
    paused: bool,
    confirm: bool,
    confirm_yes: bool,
}
impl Ui {
    fn key(&mut self, key: event::KeyEvent, urls: &[String]) -> bool {
        if self.confirm {
            match key.code {
                KeyCode::Esc | KeyCode::Char('n') => self.confirm = false,
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    self.confirm_yes = !self.confirm_yes
                }
                KeyCode::Char('y') => return true,
                KeyCode::Enter if self.confirm_yes => return true,
                KeyCode::Enter => self.confirm = false,
                _ => {}
            }
            return false;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('c')
                if key.code == KeyCode::Char('q')
                    || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.confirm = true;
                self.confirm_yes = false;
            }
            KeyCode::Tab | KeyCode::Down => self.selected = (self.selected + 1) % urls.len(),
            KeyCode::Up => self.selected = (self.selected + urls.len() - 1) % urls.len(),
            KeyCode::Enter => open_url(&urls[self.selected]),
            KeyCode::Char('s') => open_url(&format!("{}/settings", urls[0])),
            KeyCode::Char(' ') => self.paused = !self.paused,
            KeyCode::PageUp => {
                self.paused = true;
                self.offset = self.offset.saturating_add(10);
            }
            KeyCode::PageDown => self.offset = self.offset.saturating_sub(10),
            KeyCode::End => {
                self.offset = 0;
                self.paused = false;
            }
            _ => {}
        }
        false
    }
}
fn open_url(url: &str) {
    if let Err(error) = open::that_detached(url) {
        tracing::warn!(%error, "打开浏览器失败，请复制地址访问");
    }
}

pub fn run(urls: Vec<String>, logs: Logs, stop: Arc<AtomicBool>) -> io::Result<()> {
    enable_raw_mode()?;
    let _restore = RestoreTerminal;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        event::EnableMouseCapture
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut ui = Ui::default();
    let mut frozen = VecDeque::new();
    let mut address_rect = Rect::default();
    while !stop.load(Ordering::SeqCst) {
        if !ui.paused {
            frozen = logs.lock().unwrap().clone();
        }
        terminal.draw(|frame| {
            let areas = Layout::vertical([Constraint::Length(urls.len() as u16 + 2), Constraint::Min(3), Constraint::Length(1)]).split(frame.area());
            address_rect = areas[0];
            let items: Vec<ListItem> = urls.iter().enumerate().map(|(i, url)| ListItem::new(format!("{} {}  {}", if i == ui.selected { "›" } else { " " }, if i == 0 { "本机" } else { "局域网" }, url)).style(if i == ui.selected { Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD) } else { Style::default() })).collect();
            frame.render_widget(List::new(items).block(Block::default().borders(Borders::ALL).title(format!(" Cocos Build LAN v{} · 单击地址打开 ", env!("CARGO_PKG_VERSION")))), areas[0]);
            let height = areas[1].height.saturating_sub(2) as usize;
            let end = frozen.len().saturating_sub(ui.offset.min(frozen.len()));
            let lines: Vec<Line> = frozen.iter().take(end).skip(end.saturating_sub(height)).map(|s| Line::from(s.as_str())).collect();
            frame.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(if ui.paused { " 运行日志 · 已暂停 " } else { " 运行日志 · 跟随 " })), areas[1]);
            frame.render_widget(Paragraph::new("Tab/↑↓ 选择地址  Enter 打开  S 设置  空格 暂停  PgUp/PgDn 翻页  End 跟随  Q/Ctrl+C 退出"), areas[2]);
            if ui.confirm {
                let rect = centered(frame.area());
                frame.render_widget(Clear, rect);
                let text = format!("确认停止所有任务并退出？\n当前构建和准备脚本将被取消。\n\n{}    {}", if ui.confirm_yes { " 取消 " } else { "[取消]" }, if ui.confirm_yes { "[停止并退出]" } else { " 停止并退出 " });
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }).block(Block::default().borders(Borders::ALL).title(" 退出确认 · ←→选择 / Enter确认 / Esc取消 ")), rect);
            }
        })?;
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if ui.key(key, &urls) {
                        break;
                    }
                }
                Event::Mouse(mouse) if !ui.confirm => match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left)
                        if mouse.column > address_rect.x
                            && mouse.column < address_rect.right().saturating_sub(1)
                            && mouse.row > address_rect.y
                            && mouse.row < address_rect.bottom().saturating_sub(1) =>
                    {
                        let index = (mouse.row - address_rect.y - 1) as usize;
                        if let Some(url) = urls.get(index) {
                            ui.selected = index;
                            open_url(url);
                        }
                    }
                    MouseEventKind::ScrollUp => {
                        ui.paused = true;
                        ui.offset = ui.offset.saturating_add(3);
                    }
                    MouseEventKind::ScrollDown => ui.offset = ui.offset.saturating_sub(3),
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(())
}
fn centered(area: Rect) -> Rect {
    let width = area.width.min(66);
    let height = area.height.min(7);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quit_defaults_to_cancel_and_requires_explicit_confirmation() {
        let urls = vec!["http://127.0.0.1:44588".to_owned()];
        let mut ui = Ui::default();
        let key = |code| event::KeyEvent::new(code, KeyModifiers::NONE);
        assert!(!ui.key(key(KeyCode::Char('q')), &urls));
        assert!(ui.confirm && !ui.confirm_yes);
        assert!(!ui.key(key(KeyCode::Enter), &urls));
        assert!(!ui.confirm);
        ui.key(key(KeyCode::Char('q')), &urls);
        ui.key(key(KeyCode::Right), &urls);
        assert!(ui.key(key(KeyCode::Enter), &urls));
    }
}
