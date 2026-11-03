//! Game-style terminal UI: session cards with pixel scenes, HUD, preview, settings.

pub mod card;
pub mod grid;
pub mod groups;
pub mod hud;
pub mod preview;
pub mod scene;
pub mod screen;
pub mod settings;
pub mod style;

#[cfg(test)]
pub(crate) mod test_util {
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::{Frame, Terminal};
    use unicode_width::UnicodeWidthStr;

    /// Renders `ui` into a fresh `w`x`h` test terminal and returns its buffer.
    pub fn draw(w: u16, h: u16, ui: impl FnOnce(&mut Frame)) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(ui).unwrap();
        terminal.backend().buffer().clone()
    }

    /// Buffer rows as strings; the hidden cell after a wide char is skipped.
    pub fn rows(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (area.y..area.y + area.height)
            .map(|y| {
                let mut row = String::new();
                let mut x = area.x;
                while x < area.x + area.width {
                    let sym = buf[(x, y)].symbol();
                    row.push_str(sym);
                    x += sym.width().max(1) as u16;
                }
                row
            })
            .collect()
    }
}
