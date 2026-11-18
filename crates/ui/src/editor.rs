//! A small multi-line plain-text editor (wrapping, selection, IME, clipboard),
//! adapted from GPUI's `examples/input.rs` (Apache-2.0).

use crate::theme::Theme;
use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context, CursorStyle,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, SharedString, Style, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

actions!(
    zwisper_editor,
    [
        Backspace, Delete, Left, Right, Up, Down, SelectLeft, SelectRight, SelectUp, SelectDown, SelectAll,
        Home, End, Newline, Paste, Cut, Copy, Escape, WordLeft, WordRight, DeleteWordLeft
    ]
);

const CONTEXT: &str = "ZwisperEditor";

pub fn bind_keys(cx: &mut App) {
    // Key bindings need 'static strings. cmd_or_ctrl! is ⌘ on macOS, Ctrl
    // elsewhere; word movement is ⌥ on macOS, Ctrl elsewhere.
    let mac = cfg!(target_os = "macos");
    let (word_back, word_left, word_right) =
        if mac { ("alt-backspace", "alt-left", "alt-right") } else { ("ctrl-backspace", "ctrl-left", "ctrl-right") };
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new(word_back, DeleteWordLeft, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new(word_left, WordLeft, Some(CONTEXT)),
        KeyBinding::new(word_right, WordRight, Some(CONTEXT)),
        KeyBinding::new("up", Up, Some(CONTEXT)),
        KeyBinding::new("down", Down, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(CONTEXT)),
        KeyBinding::new(cmd_or_ctrl!("a"), SelectAll, Some(CONTEXT)),
        KeyBinding::new(cmd_or_ctrl!("v"), Paste, Some(CONTEXT)),
        KeyBinding::new(cmd_or_ctrl!("c"), Copy, Some(CONTEXT)),
        KeyBinding::new(cmd_or_ctrl!("x"), Cut, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new(cmd_or_ctrl!("left"), Home, Some(CONTEXT)),
        KeyBinding::new(cmd_or_ctrl!("right"), End, Some(CONTEXT)),
        KeyBinding::new("enter", Newline, Some(CONTEXT)),
        KeyBinding::new("escape", Escape, Some(CONTEXT)),
    ]);
}

pub enum EditorEvent {
    /// Content changed (every keystroke).
    Changed,
    /// Enter in single-line mode, or focus left after edits.
    Submit,
    Cancel,
}

pub struct TextEditor {
    focus: FocusHandle,
    content: String,
    placeholder: SharedString,
    multiline: bool,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    /// Layout cache: (paragraph start offset, wrapped line, y offset).
    layout: Vec<(usize, std::rc::Rc<WrappedLine>, Pixels)>,
    bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    selecting: bool,
    dirty: bool,
    /// Preferred x when moving up/down.
    goal_x: Option<Pixels>,
}

impl EventEmitter<EditorEvent> for TextEditor {}

impl Focusable for TextEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextEditor {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>, multiline: bool) -> Self {
        TextEditor {
            focus: cx.focus_handle(),
            content: String::new(),
            placeholder: placeholder.into(),
            multiline,
            selected: 0..0,
            reversed: false,
            marked: None,
            layout: Vec::new(),
            bounds: None,
            line_height: px(20.),
            selecting: false,
            dirty: false,
            goal_x: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// Replace content without emitting `Changed` (e.g. loading a session).
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.content == text {
            return;
        }
        let text = if self.multiline { text.to_string() } else { text.replace('\n', " ") };
        self.content = text;
        let end = self.content.len();
        self.selected = end..end;
        self.marked = None;
        self.dirty = false;
        cx.notify();
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.goal_x = None;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn cursor(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        self.reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.reversed { self.selected.start = offset } else { self.selected.end = offset }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn prev_boundary(&self, o: usize) -> usize {
        self.content.grapheme_indices(true).rev().find_map(|(i, _)| (i < o).then_some(i)).unwrap_or(0)
    }

    fn next_boundary(&self, o: usize) -> usize {
        self.content.grapheme_indices(true).find_map(|(i, _)| (i > o).then_some(i)).unwrap_or(self.content.len())
    }

    fn prev_word(&self, o: usize) -> usize {
        self.content
            .unicode_word_indices()
            .rev()
            .find_map(|(i, _)| (i < o).then_some(i))
            .unwrap_or(0)
    }

    fn next_word(&self, o: usize) -> usize {
        self.content
            .unicode_word_indices()
            .find_map(|(i, w)| (i + w.len() > o).then_some(i + w.len()))
            .unwrap_or(self.content.len())
    }

    // ── position mapping ──

    fn position_for(&self, offset: usize) -> Option<Point<Pixels>> {
        let (start, line, y) = self
            .layout
            .iter()
            .rev()
            .find(|(s, _, _)| *s <= offset)?;
        let local = (offset - start).min(line.len());
        let p = line.position_for_index(local, self.line_height)?;
        Some(point(p.x, p.y + *y))
    }

    fn offset_for(&self, pos: Point<Pixels>) -> usize {
        let Some(bounds) = self.bounds else { return 0 };
        let local = pos - bounds.origin;
        if self.layout.is_empty() {
            return 0;
        }
        if local.y < px(0.) {
            return 0;
        }
        for (i, (start, line, y)) in self.layout.iter().enumerate() {
            let h = line.size(self.line_height).height;
            let last = i + 1 == self.layout.len();
            if local.y < *y + h || last {
                let p = point(local.x.max(px(0.)), (local.y - *y).max(px(0.)).min(h - px(1.)));
                let ix = match line.closest_index_for_position(p, self.line_height) {
                    Ok(ix) | Err(ix) => ix,
                };
                return (start + ix).min(self.content.len());
            }
        }
        self.content.len()
    }

    fn vertical(&mut self, dir: f32, select: bool, cx: &mut Context<Self>) {
        let Some(p) = self.position_for(self.cursor()) else { return };
        let x = *self.goal_x.get_or_insert(p.x);
        let target = point(x, p.y + self.line_height * dir + self.line_height / 2.);
        let target = target + self.bounds.map(|b| b.origin).unwrap_or_default();
        let o = if dir < 0. && p.y < self.line_height / 2. {
            0
        } else {
            self.offset_for(target)
        };
        let goal = self.goal_x;
        if select { self.select_to(o, cx) } else { self.move_to(o, cx) }
        self.goal_x = goal;
    }

    // ── actions ──

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let o = if self.selected.is_empty() { self.prev_boundary(self.cursor()) } else { self.selected.start };
        self.goal_x = None;
        self.move_to(o, cx);
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let o = if self.selected.is_empty() { self.next_boundary(self.cursor()) } else { self.selected.end };
        self.goal_x = None;
        self.move_to(o, cx);
    }
    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.prev_word(self.cursor()), cx);
    }
    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor()), cx);
    }
    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1., false, cx);
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1., false, cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.prev_boundary(self.cursor()), cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor()), cx);
    }
    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1., true, cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1., true, cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor();
        let o = self.content[..c].rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.move_to(o, cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let c = self.cursor();
        let o = self.content[c..].find('\n').map(|i| c + i).unwrap_or(self.content.len());
        self.move_to(o, cx);
    }
    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.prev_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn delete_word_left(&mut self, _: &DeleteWordLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.prev_word(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.next_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.replace_text_in_range(None, "\n", window, cx);
        } else {
            self.dirty = false;
            cx.emit(EditorEvent::Submit);
        }
    }
    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::Cancel);
        window.blur(cx);
    }
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            let text = if self.multiline { text } else { text.replace('\n', " ") };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected.clone()].to_string()));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn on_mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.selecting = true;
        let o = self.offset_for(e.position);
        if e.click_count >= 2 {
            // Select the word under the cursor.
            let start = self.prev_word(o.min(self.content.len()) + 1).min(o);
            let end = self.next_word(o);
            self.move_to(start, cx);
            self.select_to(end, cx);
            return;
        }
        if e.modifiers.shift { self.select_to(o, cx) } else { self.move_to(o, cx) }
    }
    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }
    fn on_mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.select_to(self.offset_for(e.position), cx);
        }
    }

    // utf16 helpers
    fn from16(&self, o: usize) -> usize {
        let (mut u8o, mut u16c) = (0, 0);
        for ch in self.content.chars() {
            if u16c >= o {
                break;
            }
            u16c += ch.len_utf16();
            u8o += ch.len_utf8();
        }
        u8o
    }
    fn to16(&self, o: usize) -> usize {
        let (mut u16o, mut u8c) = (0, 0);
        for ch in self.content.chars() {
            if u8c >= o {
                break;
            }
            u8c += ch.len_utf8();
            u16o += ch.len_utf16();
        }
        u16o
    }
    fn range_to16(&self, r: &Range<usize>) -> Range<usize> {
        self.to16(r.start)..self.to16(r.end)
    }
    fn range_from16(&self, r: &Range<usize>) -> Range<usize> {
        self.from16(r.start)..self.from16(r.end)
    }
}

impl EntityInputHandler for TextEditor {
    fn text_for_range(&mut self, r: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let r = self.range_from16(&r);
        actual.replace(self.range_to16(&r));
        Some(self.content[r].to_string())
    }
    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to16(&self.selected), reversed: self.reversed })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to16(r))
    }
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }
    fn replace_text_in_range(&mut self, r: Option<Range<usize>>, new: &str, _: &mut Window, cx: &mut Context<Self>) {
        let new = if self.multiline { new.to_string() } else { new.replace('\n', " ") };
        let range = r.as_ref().map(|r| self.range_from16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.content = format!("{}{}{}", &self.content[..range.start], new, &self.content[range.end..]);
        let c = range.start + new.len();
        self.selected = c..c;
        self.reversed = false;
        self.marked = None;
        self.changed(cx);
    }
    fn replace_and_mark_text_in_range(&mut self, r: Option<Range<usize>>, new: &str, sel: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let range = r.as_ref().map(|r| self.range_from16(r)).or(self.marked.clone()).unwrap_or(self.selected.clone());
        self.content = format!("{}{}{}", &self.content[..range.start], new, &self.content[range.end..]);
        self.marked = if new.is_empty() { None } else { Some(range.start..range.start + new.len()) };
        self.selected = sel
            .as_ref()
            .map(|s| self.range_from16(s))
            .map(|s| s.start + range.start..s.end + range.start)
            .unwrap_or(range.start + new.len()..range.start + new.len());
        self.changed(cx);
    }
    fn bounds_for_range(&mut self, r: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let r = self.range_from16(&r);
        let a = self.position_for(r.start)?;
        let b = self.position_for(r.end)?;
        Some(Bounds::from_corners(
            bounds.origin + a,
            bounds.origin + point(b.x.max(a.x + px(1.)), b.y + self.line_height),
        ))
    }
    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.to16(self.offset_for(p)))
    }
}

struct EditorElement {
    editor: Entity<TextEditor>,
}

struct Prepaint {
    lines: Vec<(usize, std::rc::Rc<WrappedLine>, Pixels)>,
    quads: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

fn shape(editor: &TextEditor, width: Pixels, window: &mut Window, cx: &App) -> Vec<(usize, std::rc::Rc<WrappedLine>, Pixels)> {
    let theme = Theme::get(cx);
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let line_height = window.line_height();
    let (text, color) = if editor.content.is_empty() {
        (editor.placeholder.to_string(), theme.text_3)
    } else {
        (editor.content.clone(), style.color)
    };
    let mut out = Vec::new();
    let mut y = px(0.);
    let mut start = 0;
    for para in text.split('\n') {
        let base = TextRun { len: para.len(), font: style.font(), color, background_color: None, underline: None, strikethrough: None };
        let mut runs = vec![base.clone()];
        if let Some(m) = editor.marked.as_ref().filter(|_| !editor.content.is_empty()) {
            let (ms, me) = (m.start.max(start), m.end.min(start + para.len()));
            if ms < me {
                runs = vec![
                    TextRun { len: ms - start, ..base.clone() },
                    TextRun { len: me - ms, underline: Some(UnderlineStyle { color: Some(color), thickness: px(1.), wavy: false }), ..base.clone() },
                    TextRun { len: start + para.len() - me, ..base.clone() },
                ];
                runs.retain(|r| r.len > 0);
            }
        }
        let shaped = window
            .text_system()
            .shape_text(SharedString::from(para.to_string()), font_size, &runs, Some(width), None)
            .ok()
            .and_then(|mut v| v.pop())
            .unwrap_or_default();
        let h = shaped.size(line_height).height.max(line_height);
        out.push((start, std::rc::Rc::new(shaped), y));
        y += h;
        start += para.len() + 1;
    }
    out
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, _cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let editor = self.editor.clone();
        let id = window.request_measured_layout(style, move |known, avail, window, cx| {
            let width = known.width.unwrap_or(match avail.width {
                gpui::AvailableSpace::Definite(w) => w,
                _ => px(400.),
            });
            let ed = editor.read(cx);
            let lines = shape(ed, width, window, cx);
            let lh = window.line_height();
            let h = lines.last().map(|(_, l, y)| *y + l.size(lh).height.max(lh)).unwrap_or(lh);
            size(width, h)
        });
        (id, ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Prepaint {
        let lh = window.line_height();
        let theme = Theme::get(cx);
        let lines = {
            let ed = self.editor.read(cx);
            shape(ed, bounds.size.width, window, cx)
        };
        self.editor.update(cx, |e, _| {
            e.layout = lines.clone();
            e.line_height = lh;
            e.bounds = Some(bounds);
        });
        let ed = self.editor.read(cx);
        let mut quads = Vec::new();
        let mut cursor = None;
        if ed.content.is_empty() {
            cursor = Some(fill(Bounds::new(bounds.origin, size(px(1.5), lh)), theme.cursor));
        } else if ed.selected.is_empty() {
            if let Some(p) = ed.position_for(ed.cursor()) {
                cursor = Some(fill(Bounds::new(bounds.origin + p, size(px(1.5), lh)), theme.cursor));
            }
        } else {
            // Selection: one rect per visual line between start and end.
            let (s, e) = (ed.selected.start, ed.selected.end);
            if let (Some(a), Some(b)) = (ed.position_for(s), ed.position_for(e)) {
                let mut y = a.y;
                while y <= b.y {
                    let x0 = if y == a.y { a.x } else { px(0.) };
                    let x1 = if y == b.y { b.x } else { bounds.size.width };
                    if x1 > x0 || y != b.y {
                        quads.push(fill(
                            Bounds::from_corners(bounds.origin + point(x0, y), bounds.origin + point(x1.max(x0 + px(4.)), y + lh)),
                            theme.selection,
                        ));
                    }
                    y += lh;
                }
            }
        }
        Prepaint { lines, quads, cursor }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), pp: &mut Prepaint, window: &mut Window, cx: &mut App) {
        let focus = self.editor.read(cx).focus.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        for q in pp.quads.drain(..) {
            window.paint_quad(q);
        }
        let lh = window.line_height();
        for (_, line, y) in &pp.lines {
            let _ = line.paint(bounds.origin + point(px(0.), *y), lh, TextAlign::Left, None, window, cx);
        }
        if focus.is_focused(window) {
            if let Some(c) = pp.cursor.take() {
                window.paint_quad(c);
            }
        }
    }
}

impl Render for TextEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .w_full()
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(EditorElement { editor: cx.entity() })
    }
}
