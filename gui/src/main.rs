use haven::winit::WinitApp;
use haven::*;
use membank::{Bank, Kind, LIMIT, Node};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::ops::Range;
use std::path::PathBuf;
use std::process;

struct State {
    dir: PathBuf,
    bank: Bank,
    lines: Vec<Node>,
    built: usize,
    path: Vec<Node>,
    locked: bool,
    tip: Option<Node>,
    span: Range<f32>,
    ladder_scroll: f32,
    query: TextState,
    hits: Vec<(Node, String)>,
    watcher: Option<RecommendedWatcher>,
}

#[derive(Clone, Copy, PartialEq)]
enum Class {
    Line,
    Reachable,
    Coarser,
    Unbuilt,
}

const BG: Color = Color::from_rgb8(0x05, 0x07, 0x0a);
const PANEL: Color = Color::from_rgb8(0x0a, 0x0f, 0x14);
const PANEL_2: Color = Color::from_rgb8(0x0e, 0x15, 0x1c);
const LINE: Color = Color::from_rgb8(0x1b, 0x27, 0x31);
const LINE_HI: Color = Color::from_rgb8(0x2b, 0x3d, 0x4a);
const AMBER: Color = Color::from_rgb8(0xff, 0x9a, 0x1f);
const AMBER_DIM: Color = Color::from_rgb8(0x7a, 0x4a, 0x10);
const CYAN: Color = Color::from_rgb8(0x5f, 0xe3, 0xe8);
const RED: Color = Color::from_rgb8(0xff, 0x3b, 0x3b);
const TEXT: Color = Color::from_rgb8(0xd6, 0xe1, 0xe5);
const MUTE: Color = Color::from_rgb8(0x6d, 0x83, 0x90);
const FAINT: Color = Color::from_rgb8(0x3a, 0x4c, 0x57);
const DISPLAY: &str = "Avenir Next Condensed";
const MONO: &str = "Menlo";
const JP: &str = "Hiragino Mincho ProN";
const ROW: f32 = 17.;
const CARD: f32 = 98.;
const WEDGE: f32 = 26.;
const HINT: f32 = 50.;
const PITCH: f32 = CARD + WEDGE + 12.;
const EXACT: u32 = 1 << 16;
const NUMERALS: [&str; 10] = ["零", "壱", "弐", "参", "肆", "伍", "陸", "漆", "捌", "玖"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

impl State {
    fn total(&self) -> u32 {
        self.bank.messages.len() as u32
    }

    fn top(&self) -> u8 {
        self.total().checked_ilog2().unwrap_or(0) as u8
    }

    fn line_of(&self, message: u32) -> Node {
        self.lines[self.lines.partition_point(|n| n.end() <= message).min(self.lines.len() - 1)]
    }

    fn route(&self, node: Node) -> Vec<Node> {
        let mut path = vec![self.line_of(node.start())];
        while let Some(&last) = path.last().filter(|n| n.0 > node.0) {
            let [a, b] = last.children().expect("a line above level 0 has children");
            path.push(if node.start() < b.start() { a } else { b });
        }
        path
    }

    fn class(&self, node: Node) -> Class {
        let line = self.line_of(node.start());
        match () {
            _ if node.0 > line.0 => Class::Coarser,
            _ if self.bank.text(node).is_none() => Class::Unbuilt,
            _ if node == line => Class::Line,
            _ => Class::Reachable,
        }
    }

    fn color(&self, l: u8) -> Color {
        let hue = 184. - 151. * l as f32 / self.top().max(1) as f32;
        let (s, li) = (0.88, 0.62);
        let c = (1. - (2. * li - 1f32).abs()) * s;
        let x = c * (1. - ((hue / 60.) % 2. - 1.).abs());
        let (r, g, b) = match (hue / 60.) as u32 {
            0 => (c, x, 0.),
            1 => (x, c, 0.),
            2 => (0., c, x),
            _ => (0., x, c),
        };
        let m = li - c / 2.;
        Color::from_rgb8(((r + m) * 255.) as u8, ((g + m) * 255.) as u8, ((b + m) * 255.) as u8)
    }

    fn window(&self) -> Range<f32> {
        self.span.start..self.span.end.min(self.total() as f32)
    }

    fn zoom(&mut self, delta: ScrollDelta, width: f32) {
        let delta = match delta.x.abs() > delta.y.abs() {
            true => ScrollDelta { x: delta.x, y: 0. },
            false => ScrollDelta { x: 0., y: delta.y },
        };
        let Range { start, end } = self.window();
        let total = self.total() as f32;
        let len = ((end - start) * (delta.y * 0.01).exp()).clamp(8f32.min(total), total);
        let anchor = self.tip.map_or((start + end) / 2., |n| (n.start() + n.end()) as f32 / 2.).clamp(start, end);
        let start = (anchor - (anchor - start) * len / (end - start) - delta.x * (end - start) / width).clamp(0., total - len);
        self.span = start..if start + len >= total { f32::INFINITY } else { start + len };
    }

    fn go(&mut self, node: Node) {
        self.path = self.route(node);
        self.reveal();
    }

    fn reveal(&mut self) {
        self.ladder_scroll = self.path.len().saturating_sub(2) as f32 * PITCH;
    }

    fn find(&mut self, query: &str) {
        self.hits = self.bank.search(query).into_iter().map(|(node, text)| (node, clip(text, 52))).collect();
    }

    fn select(&mut self, rung: usize, node: Node) {
        if self.path.get(rung) == Some(&node) {
            return;
        }
        self.path.truncate(rung);
        self.path.push(node);
    }

    fn step(&mut self, key: &Key) {
        let last = *self.path.last().expect("the path always has its line");
        let Key::Named(key) = key else { return };
        match key {
            NamedKey::Escape => {
                self.locked = false;
                return;
            }
            NamedKey::ArrowDown => self.path.extend(last.children().map(|[a, _]| a)),
            NamedKey::ArrowUp => {
                self.path.truncate((self.path.len() - 1).max(1));
            }
            NamedKey::ArrowLeft | NamedKey::ArrowRight => {
                let right = *key == NamedKey::ArrowRight;
                match self.path.len() {
                    1 => {
                        let at = self.lines.iter().position(|&n| n == last).unwrap_or(0);
                        if let Some(&next) = if right { self.lines.get(at + 1) } else { at.checked_sub(1).and_then(|i| self.lines.get(i)) } {
                            self.path = vec![next];
                        }
                    }
                    _ => {
                        if let Some(i) = if right { Some(last.1 + 1) } else { last.1.checked_sub(1) }.filter(|&i| i < self.total() >> last.0) {
                            self.path = self.route(Node(last.0, i));
                        }
                    }
                }
            }
            _ => return,
        }
        self.locked = true;
        self.reveal();
    }
}

fn load(bank: Bank) -> (Bank, Vec<Node>, usize) {
    let covered = bank.view.last().map_or(0, |n| n.end());
    let lines = bank.view.iter().copied().chain((covered..bank.messages.len() as u32).map(|i| Node(0, i))).collect();
    let built = bank.nodes().count();
    (bank, lines, built)
}

fn moment(date: &str) -> String {
    let month = date.get(5..7).and_then(|m| m.parse::<usize>().ok()).and_then(|m| MONTHS.get(m.wrapping_sub(1)));
    match (month, date.get(8..10), date.get(11..16)) {
        (Some(month), Some(day), Some(time)) => format!("{month} {day} {time}"),
        _ => date.to_string(),
    }
}

fn clip(text: &str, chars: usize) -> String {
    let flat = text.replace('\n', " ");
    match flat.char_indices().nth(chars) {
        Some((cut, _)) => format!("{}…", flat[..cut].trim_end()),
        None => flat,
    }
}

fn fit(text: &str, columns: usize, lines: usize) -> String {
    let columns = columns.max(1);
    let mut out = String::new();
    let (mut line, mut column) = (1, 0);
    for word in text.split_whitespace() {
        let len = word.chars().count();
        let end = if column == 0 { len } else { column + 1 + len };
        if end > columns {
            line += 1 + len / columns;
            column = len % columns;
            if line > lines {
                return format!("{out}…");
            }
        } else {
            column = end;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

fn key(node: Node) -> u64 {
    (node.0 as u64) << 32 | node.1 as u64
}

fn mono<'a>(id: u64, body: impl AsRef<str>, size: u32, color: Color, app: &mut PaneState) -> View<'a, State> {
    text(id, body).font_family(MONO).font_size(size).fill(color).build(app)
}

fn display<'a>(id: u64, body: impl AsRef<str>, size: u32, color: Color, app: &mut PaneState) -> View<'a, State> {
    text(id, body).font_family(DISPLAY).font_weight(FontWeight::BOLD).font_size(size).fill(color).build(app)
}

fn jp<'a>(id: u64, body: impl AsRef<str>, size: u32, app: &mut PaneState) -> View<'a, State> {
    text(id, body).font_family(JP).font_weight(FontWeight::BLACK).font_size(size).fill(AMBER).build(app)
}

fn square(id: u64) -> Rect {
    rect(id).corner_rounding(0.)
}

fn chamfer(a: Area, cut: f64) -> BezPath {
    let (x, y, w, h) = (a.x as f64, a.y as f64, a.width as f64, a.height as f64);
    let mut p = BezPath::new();
    p.move_to((x, y));
    p.line_to((x + w - cut, y));
    p.line_to((x + w, y + cut));
    p.line_to((x + w, y + h));
    p.line_to((x + cut, y + h));
    p.line_to((x, y + h - cut));
    p.close_path();
    p
}

fn placed<'a>(view: View<'a, State>, area: Area, x: f32, y: f32, w: f32, h: f32) -> View<'a, State> {
    view.width(w).height(h).offset(x + w / 2. - area.width / 2., y + h / 2. - area.height / 2.)
}

fn on_hover(id: u64, run: impl Fn(&mut State) + 'static) -> Gesture<State> {
    gesture::hover(id).run(move |state: &mut State, app, inside| {
        if inside && !state.locked {
            run(state);
            app.redraw();
        }
    })
}

fn on_click(id: u64, run: impl Fn(&mut State) + 'static) -> Gesture<State> {
    gesture::click(id).button(MouseButton::Left).run(move |state: &mut State, app, event| {
        if event.state == ClickPhase::Completed {
            run(state);
            app.redraw();
        }
    })
}

fn panel<'a>(id: u64, title: &str, glyphs: &str, meta: String, body: View<'a, State>, app: &mut PaneState) -> View<'a, State> {
    stack(vec![
        square(id!(id)).fill(PANEL).stroke(LINE, Stroke::new(1.)).build(app),
        column(vec![
            row_spaced(
                10.,
                vec![display(id!(id), title.to_uppercase(), 21, TEXT, app), jp(id!(id), glyphs, 13, app), space(), mono(id!(id), meta, 10, MUTE, app)],
            )
            .height(36.)
            .pad_x(12.),
            square(id!(id)).fill(LINE).build(app).height(1.),
            body.expand(),
        ]),
        path(id!(id), |a| {
            let mut p = BezPath::new();
            let (x, y, r, b) = (a.x as f64, a.y as f64, (a.x + a.width) as f64, (a.y + a.height) as f64);
            p.move_to((x, y + 12.));
            p.line_to((x, y));
            p.line_to((x + 12., y));
            p.move_to((r - 12., b));
            p.line_to((r, b));
            p.line_to((r, b - 12.));
            p
        })
        .stroke(AMBER, Stroke::new(2.))
        .build(app),
    ])
}

fn chip<'a>(id: u64, label: &str, value: String, color: Color, border: Color, dot: Option<Color>, app: &mut PaneState) -> View<'a, State> {
    let mut parts = vec![mono(id!(id), label, 11, MUTE, app), mono(id!(id), value.as_str(), 11, color, app)];
    if let Some(dot) = dot {
        parts.insert(0, stack(vec![shadow(id!(id)).color(dot).blur(8.).corner_rounding(3.5).build(app), circle(id!(id)).fill(dot).finish(app)]).width(7.).height(7.));
    }
    stack(vec![square(id!(id)).fill(PANEL).stroke(border, Stroke::new(1.)).build(app), row_spaced(6., parts)])
        .width((label.len() + value.chars().count() + 1) as f32 * 6.7 + 20. + if dot.is_some() { 13. } else { 0. })
        .height(24.)
}

fn header<'a>(state: &'a State, app: &mut PaneState) -> View<'a, State> {
    let total = state.total();
    let bytes: usize = state.lines.iter().map(|&n| state.bank.line(n).len() + 1).sum();
    let complete: usize = (0..=state.top()).map(|l| (total >> l) as usize).sum();
    let pending = complete.saturating_sub(state.built);
    let (link, link_color) = if state.locked { ("LOCKED", RED) } else { ("TRACKING", CYAN) };
    let crest = stack(vec![
        path(id!(), |a| {
            let mut p = BezPath::new();
            let (x, y, w, h) = (a.x as f64, a.y as f64, a.width as f64, a.height as f64);
            p.move_to((x, y));
            p.line_to((x + w, y));
            p.line_to((x + w - 14., y + h));
            p.line_to((x, y + h));
            p.close_path();
            p
        })
        .fill(AMBER)
        .build(app),
        row_spaced(
            12.,
            vec![
                text(id!(), "記憶樹").font_family(JP).font_weight(FontWeight::BLACK).font_size(30).fill(BG).build(app),
                column(vec![
                    display(id!(), "MEMBANK", 26, BG, app).align(Align::Leading),
                    text(id!(), "T R E E   N A V I G A T O R").font_family(DISPLAY).font_weight(FontWeight::SEMI_BOLD).font_size(10).fill(BG).build(app).align(Align::Leading),
                ])
                .width(150.),
            ],
        )
        .pad_leading(10.)
        .pad_trailing(24.),
    ])
    .width(300.)
    .height(48.);
    let hazard = path(id!(), |a| {
        let mut p = BezPath::new();
        let (y, h) = (a.y as f64 + a.height as f64 / 2. - 5., 10.);
        let mut x = a.x as f64;
        while x < (a.x + a.width) as f64 - 8. {
            p.move_to((x + h, y));
            p.line_to((x + h + 8., y));
            p.line_to((x + 8., y + h));
            p.line_to((x, y + h));
            p.close_path();
            x += 16.;
        }
        p
    })
    .fill(AMBER_DIM.with_alpha(0.7))
    .build(app);
    let lock = stack(vec![
        chip(id!(), "LINK", link.to_string(), link_color, if state.locked { RED } else { LINE_HI }, Some(link_color), app),
        square(id!()).fill(TRANSPARENT).view().gesture(on_click(id!(), |s| s.locked = !s.locked)).build(app),
    ])
    .width((4 + link.len() + 1) as f32 * 6.7 + 33.)
    .height(24.);
    row_spaced(
        8.,
        vec![
            crest,
            hazard.expand_x().height(48.),
            chip(id!(), "LOG", format!("{total} MSG"), TEXT, LINE_HI, None, app),
            chip(id!(), "TREE", format!("L0–L{} · {} NODES", state.top(), state.built), TEXT, LINE_HI, None, app),
            chip(id!(), "VIEW", format!("{} NODES · {:.1} KB", state.lines.len(), bytes as f32 / 1024.), TEXT, LINE_HI, None, app),
            match pending {
                0 => chip(id!(), "COMPACTOR", "SYNCED".to_string(), CYAN, LINE_HI, Some(CYAN), app),
                n => chip(id!(), "COMPACTOR", format!("{n} PENDING"), AMBER, LINE_HI, Some(CYAN), app),
            },
            lock,
        ],
    )
    .height(48.)
}

fn schematic<'a>(state: &'a State, area: Area, ctx: &mut PaneState) -> View<'a, State> {
    let Range { start: lo, end: hi } = state.window();
    let top = state.top();
    let scale = area.width / (hi - lo);
    let y = |l: u8| (top - l) as f32 * ROW;
    let x = |m: u32| (m as f32 - lo) * scale;
    let width = area.width;
    let span = |start: u32, end: u32| (x(start).max(-2.), x(end).min(width + 2.));
    let visible = |l: u8, count: u32| (lo as u32 >> l)..((hi.ceil() as u32 >> l) + 1).min(count);
    let mut views = vec![
        square(id!())
            .fill(TRANSPARENT)
            .view()
            .gesture(gesture::scroll(id!()).run(move |state: &mut State, app, delta| {
                state.zoom(delta, width);
                app.redraw();
            }))
            .build(ctx),
    ];
    let mut seams = Vec::new();
    for l in (0..=top).rev() {
        let count = state.total() >> l;
        let width = (1u32 << l) as f32 * scale;
        let mut runs: Vec<(u32, u32, Class)> = Vec::new();
        let mut push = |start: u32, end: u32, class: Class| match runs.last_mut() {
            Some(run) if run.2 == class && run.1 == start => run.1 = end,
            _ => runs.push((start, end, class)),
        };
        if count <= EXACT {
            for i in visible(l, count) {
                let node = Node(l, i);
                push(node.start(), node.end(), state.class(node));
                if width >= 3. {
                    seams.push((x(node.end()), y(l)));
                }
            }
        } else {
            for &line in &state.lines {
                let end = line.end().min(count << l);
                if line.start() < end && line.start() < hi.ceil() as u32 && lo < end as f32 {
                    let class = match l.cmp(&line.0) {
                        std::cmp::Ordering::Greater => Class::Coarser,
                        std::cmp::Ordering::Equal => Class::Line,
                        std::cmp::Ordering::Less => Class::Reachable,
                    };
                    push(line.start(), end, class);
                }
            }
        }
        let color = state.color(l);
        for (n, (start, end, class)) in runs.into_iter().enumerate() {
            let band = match class {
                Class::Line => square(id!(l as u64, n as u64)).fill(color.with_alpha(0.5)).stroke(color, Stroke::new(1.)),
                Class::Reachable => square(id!(l as u64, n as u64)).fill(color.with_alpha(0.18)),
                Class::Coarser => square(id!(l as u64, n as u64)).fill(AMBER.with_alpha(0.06)).stroke(LINE, Stroke::new(1.)),
                Class::Unbuilt => square(id!(l as u64, n as u64)).fill(TRANSPARENT).stroke(FAINT, Stroke::new(1.).with_dashes(0., [3., 3.])),
            };
            let (left, right) = span(start, end);
            views.push(placed(band.build(ctx), area, left, y(l) + 1., right - left, ROW - 2.));
        }
        let (left, right) = span(count << l, state.total());
        if left < right {
            let pend = square(id!(l as u64)).fill(TRANSPARENT).stroke(FAINT, Stroke::new(1.).with_dashes(0., [3., 3.])).build(ctx);
            views.push(placed(pend, area, left, y(l) + 1., right - left, ROW - 2.));
        }
    }
    views.push(
        path(id!(), move |a| {
            let mut p = BezPath::new();
            for (sx, sy) in &seams {
                p.move_to((a.x as f64 + *sx as f64, a.y as f64 + *sy as f64));
                p.line_to((a.x as f64 + *sx as f64, a.y as f64 + (*sy + ROW) as f64));
            }
            p
        })
        .stroke(BG, Stroke::new(1.))
        .build(ctx),
    );
    let marks: Vec<(f32, f32, f32)> = state
        .hits
        .iter()
        .filter_map(|(n, _)| {
            let (left, right) = span(n.start(), n.end());
            (left < right).then(|| (left, (right - left).max(2.), y(n.0)))
        })
        .collect();
    views.push(
        path(id!(), move |a| {
            let mut p = BezPath::new();
            for &(mx, w, my) in &marks {
                let (left, top) = (a.x as f64 + mx as f64, a.y as f64 + my as f64 + 1.);
                p.move_to((left, top));
                p.line_to((left + w as f64, top));
                p.line_to((left + w as f64, top + ROW as f64 - 2.));
                p.line_to((left, top + ROW as f64 - 2.));
                p.close_path();
            }
            p
        })
        .fill(CYAN.with_alpha(0.12))
        .stroke(CYAN, Stroke::new(1.))
        .build(ctx),
    );
    let last = *state.path.last().expect("the path always has its line");
    let mut lit = Vec::new();
    for (k, &node) in state.path.iter().enumerate() {
        lit.push((node, AMBER));
        if k > 0 {
            lit.extend(state.path[k - 1].children().into_iter().flatten().filter(|&c| c != node).map(|c| (c, state.color(c.0).with_alpha(0.6))));
        }
    }
    lit.extend(last.children().into_iter().flatten().map(|c| (c, state.color(c.0).with_alpha(0.6))));
    for (n, (node, color)) in lit.into_iter().enumerate() {
        let (left, right) = span(node.start(), node.end());
        if left < right {
            views.push(placed(square(id!(key(node), n as u64)).fill(color).stroke(BG, Stroke::new(1.)).build(ctx), area, left, y(node.0) + 1., (right - left).max(1.), ROW - 2.));
        }
    }
    if let Some(tip) = state.tip {
        let (left, right) = span(tip.start(), tip.end());
        let ring = square(id!(key(tip))).fill(TRANSPARENT).stroke(TEXT, Stroke::new(1.5)).build(ctx);
        views.push(placed(ring, area, left, y(tip.0) + 1., (right - left).max(2.), ROW - 2.));
    }
    for l in 0..=top {
        let count = state.total() >> l;
        let group = (5. / ((1u32 << l) as f32 * scale)).ceil().max(1.) as u32;
        let cells = ((lo as u32 >> l) / group)..((hi.ceil() as u32 >> l).min(count) + group - 1) / group;
        for k in cells {
            let (first, last) = (k * group, ((k + 1) * group).min(count));
            let node = Node(l, (first + last) / 2);
            let (left, right) = (x(first << l).max(0.), x(last << l).min(area.width));
            let target = square(id!(key(node), k as u64))
                .fill(TRANSPARENT)
                .view()
                .gesture(gesture::hover(id!(key(node), k as u64)).run(move |state: &mut State, app, inside| {
                    match inside {
                        true => state.tip = Some(node),
                        false if state.tip == Some(node) => state.tip = None,
                        false => return,
                    }
                    if inside && !state.locked && state.class(node) != Class::Coarser {
                        state.go(node);
                    }
                    app.redraw();
                }))
                .gesture(on_click(id!(key(node), k as u64), move |state| {
                    if state.class(node) != Class::Coarser {
                        state.go(node);
                        state.locked = !state.locked;
                    }
                }))
                .build(ctx);
            views.push(placed(target, area, left, y(l), right - left, ROW));
        }
    }
    stack(views).width(area.width).height(area.height).clipped(rect_path)
}

fn schematic_panel<'a>(state: &'a State, app: &mut PaneState) -> View<'a, State> {
    let rows = state.top() as f32 + 1.;
    let last = *state.path.last().expect("the path always has its line");
    let focus = match state.tip {
        Some(tip) => {
            let line = state.line_of(tip.start());
            let place = match state.class(tip) {
                Class::Coarser => "coarser than view · agent sees its children".to_string(),
                _ if tip == line => "view line".to_string(),
                _ => format!("inside {line}"),
            };
            format!("{tip} · L{} · {place} · {}", tip.0, clip(state.bank.text(tip).unwrap_or("not built yet"), 70))
        }
        None if last.0 == 0 => format!("focus · message #{}", last.start()),
        None => format!("focus · {last} · {} msgs", last.len()),
    };
    let legend = |id: u64, label: &str, swatch: View<'a, State>, app: &mut PaneState| {
        row_spaced(5., vec![swatch.width(12.).height(8.), mono(id!(id), label.to_uppercase(), 10, MUTE, app)]).width(label.len() as f32 * 6.1 + 22.)
    };
    let color = state.color(1);
    let labels: Vec<View<'a, State>> = (0..=state.top())
        .rev()
        .map(|l| {
            let hot = state.path.iter().any(|n| n.0 == l);
            row(vec![space(), mono(id!(l as u64), format!("L{l}"), 10, if hot { AMBER } else { FAINT }, app)]).height(ROW)
        })
        .collect();
    let ticks: Vec<View<'a, State>> = (0..=4)
        .map(|q| {
            let Range { start, end } = state.window();
            let label = mono(id!(q as u64), format!("#{}", (start + (end - start) * q as f32 / 4.) as u32), 9, FAINT, app);
            match q {
                0 => row(vec![label, space()]),
                4 => row(vec![space(), label]),
                _ => label,
            }
        })
        .collect();
    stack(vec![
        square(id!()).fill(PANEL).stroke(LINE, Stroke::new(1.)).build(app),
        column_spaced(
            6.,
            vec![
                row_spaced(
                    14.,
                    vec![
                        display(id!(), "TREE SCHEMATIC", 16, TEXT, app),
                        jp(id!(), "系統図", 12, app),
                        legend(1, "view line", square(id!()).fill(color.with_alpha(0.5)).stroke(color, Stroke::new(1.)).build(app), app),
                        legend(2, "reachable by zoom", square(id!()).fill(color.with_alpha(0.18)).build(app), app),
                        legend(3, "coarser than view", square(id!()).fill(AMBER.with_alpha(0.06)).stroke(LINE_HI, Stroke::new(1.)).build(app), app),
                        legend(4, "not built yet", square(id!()).fill(TRANSPARENT).stroke(FAINT, Stroke::new(1.).with_dashes(0., [2., 2.])).build(app), app),
                        legend(5, "hover path", square(id!()).fill(AMBER).build(app), app),
                        legend(6, "search hit", square(id!()).fill(CYAN.with_alpha(0.12)).stroke(CYAN, Stroke::new(1.)).build(app), app),
                        row(vec![space(), mono(id!(), clip(&focus, 96), 10, AMBER, app)]).expand_x().clipped(rect_path),
                    ],
                )
                .height(18.),
                row_spaced(
                    8.,
                    vec![
                        column(labels).width(40.),
                        draw(move |area, ctx: &mut PaneState| schematic(state, area, ctx).draw(area, ctx)).expand_x().height(rows * ROW),
                    ],
                ),
                row_spaced(8., vec![space().width(40.), row(ticks.into_iter().flat_map(|t| [t, space()]).take(9).collect())]).height(12.),
            ],
        )
        .pad_x(12.)
        .pad_top(8.)
        .pad_bottom(4.),
    ])
    .height(rows * ROW + 58.)
}

fn view_row<'a>(line: Node, body: &str, state: &'a State, app: &mut PaneState) -> View<'a, State> {
    let id = key(line);
    let on = state.path.contains(&line);
    let color = state.color(line.0);
    let pip = ((58. - state.top() as f32) / (state.top() as f32 + 1.)).min(5.);
    let pips: Vec<View<'a, State>> = (0..=state.top()).map(|k| square(id!(id, k as u64)).fill(if k <= line.0 { color } else { LINE_HI }).build(app).width(pip).height(4.)).collect();
    let (background, bar) = match on {
        true => (
            square(id!(id)).fill(|a: Area, _: &()| Gradient::new_linear((a.x as f64, 0.), ((a.x + a.width) as f64, 0.)).with_stops([AMBER.with_alpha(0.12), TRANSPARENT]).into()),
            stack(vec![shadow(id!(id)).color(color).blur(8.).corner_rounding(0.).build(app), square(id!(id)).fill(color).build(app)]),
        ),
        false => (square(id!(id)).fill(TRANSPARENT), square(id!(id)).fill(color.with_alpha(0.45)).build(app)),
    };
    stack(vec![
        background.view().gesture(on_hover(id!(id), move |s| s.go(line))).gesture(on_click(id!(id), move |s| s.go(line))).build(app),
        row_spaced(
            8.,
            vec![
                bar.width(4.),
                column_spaced(
                    2.,
                    vec![
                        mono(id!(id), line.to_string(), 11, color, app).align(Align::Leading),
                        mono(id!(id), format!("L{}", line.0), 9, FAINT, app).align(Align::Leading),
                        row_spaced(1., pips).align(Align::Leading),
                        space(),
                    ],
                )
                .width(62.),
                text(id!(id), clip(body, 52))
                    .font_size(12)
                    .fill(if on { TEXT } else { MUTE })
                    .align(Alignment::Start)
                    .wrap()
                    .build(app)
                    .expand_x()
                    .align(Align::Top),
            ],
        )
        .pad_y(6.)
        .pad_trailing(10.),
        column(vec![space(), square(id!(id)).fill(LINE.with_alpha(0.6)).build(app).height(1.)]),
    ])
    .height(58.)
}

fn card<'a>(rung: usize, node: Node, columns: usize, state: &'a State, app: &mut PaneState) -> View<'a, State> {
    let id = key(node);
    let color = state.color(node.0);
    let on = state.path.get(rung) == Some(&node);
    let dim = rung > 0 && state.path.get(rung).is_some_and(|&p| p != node);
    let first = &state.bank.messages[node.start() as usize];
    let last = &state.bank.messages[node.end() as usize - 1];
    let summary = state.bank.text(node);
    let bytes = summary.map_or(0, str::len);
    let mut head = vec![mono(id!(id), node.to_string(), 13, color, app)];
    let body = match node.0 {
        0 => {
            head.push(mono(id!(id), moment(&first.date), 10, MUTE, app));
            head.push(space());
            let (role, tint) = match first.kind {
                Kind::User => ("USER", AMBER),
                Kind::Assistant => ("ASSISTANT", CYAN),
            };
            head.push(stack(vec![square(id!(id)).stroke(tint, Stroke::new(1.)).build(app), mono(id!(id), role, 9, tint, app)]).width(role.len() as f32 * 6. + 12.).height(15.));
            first.text.as_str()
        }
        _ => {
            head.push(mono(id!(id), format!("#{}–{}", node.start(), node.end() - 1), 10, MUTE, app));
            let (a, b) = (moment(&first.date), moment(&last.date));
            let span = if a[..6] == b[..6] { a[..6].to_string() } else { format!("{} → {}", &a[..6], &b[..6]) };
            head.push(mono(id!(id), span, 10, MUTE, app));
            head.push(space());
            summary.unwrap_or("not summarized yet")
        }
    };
    let body = fit(body, columns, 3);
    let (surface, bar) = match on {
        true => (
            path(id!(id), |a| chamfer(a, 10.))
                .fill(|a: Area, _: &()| Gradient::new_linear((0., a.y as f64), (0., (a.y + a.height) as f64)).with_stops([(0., Color::from_rgb8(0x26, 0x22, 0x1c)), (0.7, PANEL_2)]).into())
                .stroke(AMBER, Stroke::new(1.)),
            stack(vec![shadow(id!(id)).color(color).blur(10.).corner_rounding(0.).build(app), square(id!(id)).fill(color).build(app)]),
        ),
        false => (path(id!(id), |a| chamfer(a, 10.)).fill(PANEL_2).stroke(LINE_HI, Stroke::new(1.)), square(id!(id)).fill(color.with_alpha(0.55)).build(app)),
    };
    let view = stack(vec![
        surface.build(app),
        row(vec![bar.width(3.), space()]),
        column_spaced_aligned(5., Align::Top, vec![row_spaced(8., head).height(16.), text(id!(id), body).font_size(13).fill(TEXT).align(Alignment::Start).wrap().build(app).expand_x()])
            .height(16. + 5. + 37.)
            .clipped(rect_path)
            .pad(8.)
            .pad_leading(12.)
            .pad_bottom(CARD - 16. - 16. - 5. - 37. - 8.),
        column(vec![
            space(),
            row_spaced(
                8.,
                vec![
                    stack(vec![
                        square(id!(id)).fill(LINE).build(app),
                        row(vec![square(id!(id)).fill(color.with_alpha(0.8)).build(app).width(bytes.min(LIMIT) as f32 / LIMIT as f32 * 120.), space()]),
                    ])
                    .width(120.)
                    .height(3.),
                    space(),
                    mono(id!(id), format!("{bytes}B"), 10, FAINT, app),
                ],
            )
            .height(12.),
        ])
        .pad(8.)
        .pad_leading(12.),
        square(id!(id))
            .fill(TRANSPARENT)
            .view()
            .gesture(on_click(id!(id), move |s| {
                s.select(rung, node);
                s.locked = !s.locked;
            }))
            .build(app),
    ])
    .height(CARD)
    .clipped(|a| chamfer(a, 10.));
    if dim { view.opacity(0.45) } else { view }
}

fn rung_label<'a>(l: u8, hot: bool, app: &mut PaneState) -> View<'a, State> {
    column(vec![
        row(vec![space(), display(id!(l as u64), format!("L{l}"), 22, if hot { AMBER } else { FAINT }, app)]),
        row(vec![space(), text(id!(l as u64), format!("{}層", NUMERALS.get(l as usize).copied().unwrap_or("多"))).font_family(JP).font_weight(FontWeight::BLACK).font_size(11).fill(if hot { AMBER } else { FAINT }).build(app)]),
        space(),
    ])
    .width(44.)
    .pad_top(6.)
}

fn wedge<'a>(rung: usize, state: &'a State, app: &mut PaneState) -> View<'a, State> {
    let parent = state.path[rung - 1];
    let from = if rung == 1 { 0.5 } else if parent.1 % 2 == 1 { 0.75 } else { 0.25 };
    let taken = state.path.get(rung).map(|n| n.1 % 2);
    let fork = move |side: u32| {
        move |a: Area| {
            let (x, y, w) = (a.x as f64, a.y as f64, a.width as f64);
            let to = if side == 0 { 0.25 } else { 0.75 };
            let mut p = BezPath::new();
            p.move_to((x + w * from, y + 11.));
            p.line_to((x + w * to, y + 11.));
            p.line_to((x + w * to, y + 26.));
            p
        }
    };
    let branch = |side: u32, app: &mut PaneState| {
        let on = taken == Some(side);
        path(id!(rung as u64, side as u64), fork(side)).stroke(if on { AMBER } else { LINE_HI }, Stroke::new(if on { 2.5 } else { 1.5 })).build(app)
    };
    let label = |side: u32, app: &mut PaneState| {
        let name = if side == 0 { "left half" } else { "right half" };
        mono(id!(rung as u64, side as u64), name, 9, if taken == Some(side) { AMBER } else { FAINT }, app)
    };
    row_spaced(
        8.,
        vec![
            space().width(44.),
            stack(vec![
                path(id!(rung as u64), move |a| {
                    let mut p = BezPath::new();
                    p.move_to((a.x as f64 + a.width as f64 * from, a.y as f64));
                    p.line_to((a.x as f64 + a.width as f64 * from, a.y as f64 + 11.));
                    p
                })
                .stroke(AMBER, Stroke::new(1.5))
                .build(app),
                branch(0, app),
                branch(1, app),
                row(vec![space(), label(0, app).pad_top(15.).pad_leading(70.), space(), space(), label(1, app).pad_top(15.).pad_leading(76.), space()]),
            ]),
        ],
    )
    .height(WEDGE)
}

fn ladder<'a>(state: &'a State, area: Area, ctx: &mut PaneState) -> View<'a, State> {
    let mut rungs: Vec<(usize, Vec<Node>)> = vec![(0, vec![state.path[0]])];
    while let Some(children) = state.path.get(rungs.len() - 1).and_then(|n| n.children()).filter(|_| rungs.len() <= state.path.len()) {
        rungs.push((rungs.len(), children.to_vec()));
        if state.path.len() < rungs.len() {
            break;
        }
    }
    let hidden = (state.ladder_scroll / PITCH) as usize;
    let last = *state.path.last().expect("the path always has its line");
    let card_width = (area.width - 24. - 52.) / 2.;
    let mut views = Vec::new();
    for (rung, nodes) in &rungs {
        if *rung > 0 {
            views.push(wedge(*rung, state, ctx));
        }
        let hot = *rung + 1 == state.path.len();
        let width = if nodes.len() == 1 { card_width * 2. + 6. } else { card_width };
        let columns = ((width - 28.) / 6.6) as usize;
        let cards: Vec<View<'a, State>> = nodes.iter().map(|&n| card(*rung, n, columns, state, ctx)).collect();
        views.push(row_spaced(8., vec![rung_label(nodes[0].0, hot, ctx), row_spaced(6., cards).expand_x()]).height(CARD));
    }
    let hint = match (last.0, state.path.len() == rungs.len()) {
        (0, _) => "■ VERBATIM MESSAGE REACHED · ROOT LOG".to_string(),
        (l, true) => format!("▼ {l} MORE LEVEL{} TO THE RAW LOG · HOVER TO DESCEND", if l > 1 { "S" } else { "" }),
        (l, false) => format!("▲ HOVER A HALF · {l} LEVEL{} TO THE RAW LOG", if l > 1 { "S" } else { "" }),
    };
    views.push(
        row_spaced(
            8.,
            vec![
                space().width(44.),
                stack(vec![square(id!()).stroke(LINE_HI, Stroke::new(1.).with_dashes(0., [4., 3.])).build(ctx), mono(id!(), hint, 11, MUTE, ctx)]).height(38.).expand_x(),
            ],
        )
        .pad_top(10.),
    );
    views.push(space());
    let height = rungs.len() as f32 * PITCH + HINT + 12.;
    let above = match hidden {
        0 => String::new(),
        1 => "▲ 1 rung above · scroll up".to_string(),
        n => format!("▲ {n} rungs above · scroll up"),
    };
    let window = Area { x: 0., y: 0., width: area.width - 24., height: area.height - 44. };
    let body = placed(column_spaced(6., views), window, 0., -state.ladder_scroll, window.width, height);
    column_spaced(
        6.,
        vec![
            row_spaced(8., vec![space().width(44.), mono(id!(), above, 10, FAINT, ctx), space()]).height(14.),
            stack(vec![
                square(id!())
                    .fill(TRANSPARENT)
                    .view()
                    .gesture(gesture::scroll(id!()).vertical().run(|state: &mut State, app, delta| {
                        let deepest = state.path.len() - state.path.last().expect("the path always has its line").children().is_none() as usize;
                        state.ladder_scroll = (state.ladder_scroll + delta.y).clamp(0., deepest as f32 * PITCH);
                        app.redraw();
                    }))
                    .build(ctx),
                body,
            ])
            .clipped(rect_path),
        ],
    )
    .pad(12.)
}

fn crumbs<'a>(state: &'a State, room: f32, app: &mut PaneState) -> View<'a, State> {
    let crumb_width = |node: Node| node.to_string().len() as f32 * 6.7 + 10.;
    let mut used = 24. + 20.;
    let shown = state
        .path
        .iter()
        .rev()
        .take_while(|&&node| {
            used += crumb_width(node) + 11.;
            used <= room
        })
        .count();
    let first = state.path.len() - shown.max(1);
    let mut parts = Vec::new();
    if first > 0 {
        parts.push(mono(id!(), "…", 11, FAINT, app));
    }
    for (k, &node) in state.path.iter().enumerate().skip(first) {
        if k > first || first > 0 {
            parts.push(mono(id!(k as u64), "›", 11, FAINT, app));
        }
        let name = node.to_string();
        let width = crumb_width(node);
        let label = mono(id!(key(node)), &name, 11, state.color(node.0), app);
        parts.push(match k + 1 == state.path.len() {
            true => stack(vec![square(id!(key(node))).stroke(state.color(node.0), Stroke::new(1.)).build(app), label]).width(width).height(18.),
            false => label.width(width),
        });
    }
    parts.push(space());
    column(vec![row_spaced(2., parts).height(34.).pad_x(12.), square(id!()).fill(LINE).build(app).height(1.)])
}

fn readout<'a>(state: &'a State, app: &mut PaneState) -> View<'a, State> {
    scroller(id!(), None, move |i, _, app| readout_cell(i, state, app), app)
}

fn readout_cell<'a>(i: usize, state: &'a State, app: &mut PaneState) -> Option<View<'a, State>> {
    let node = *state.path.last().expect("the path always has its line");
    let color = state.color(node.0);
    let first = &state.bank.messages[node.start() as usize];
    let last = &state.bank.messages[node.end() as usize - 1];
    let summary = state.bank.text(node);
    let bytes = summary.map_or(0, str::len);
    let leaf = node.0 == 0;
    let verbatim = leaf && summary == Some(format!("{}: {}", first.kind, first.text).as_str());
    let parent = Node(node.0 + 1, node.1 / 2);
    let line = state.line_of(node.start());
    let kv = |id: u64, label: &str, value: View<'a, State>, app: &mut PaneState| {
        row_spaced(10., vec![row(vec![mono(id!(id), label, 10, MUTE, app), space()]).width(92.).align(Align::Top), value.expand_x()])
    };
    let value = |id: u64, body: String, app: &mut PaneState| text(id!(id), body).font_size(13).fill(TEXT).align(Alignment::Start).wrap().build(app).expand_x();
    let gauge = column(vec![
        row(vec![space(), mono(id!(), LIMIT.to_string(), 9, RED, app)]).height(12.),
        stack(vec![
            path(id!(), |a| {
                let mut p = BezPath::new();
                for k in 1..10 {
                    let x = a.x as f64 + a.width as f64 * k as f64 / 10.;
                    p.move_to((x, a.y as f64));
                    p.line_to((x, (a.y + a.height) as f64));
                }
                p
            })
            .stroke(LINE, Stroke::new(1.))
            .build(app),
            row(vec![square(id!()).fill(color).build(app).width(bytes.min(LIMIT) as f32 / LIMIT as f32 * 200.), space()]),
            square(id!()).fill(TRANSPARENT).stroke(LINE_HI, Stroke::new(1.)).build(app),
        ])
        .height(10.),
    ])
    .width(200.);
    let zoom = match node.children() {
        None => format!("> zoom({}, 1)\n← verbatim · {} · {} B\n{}", node.start(), first.kind, first.text.len(), clip(&first.text, 140)),
        Some([a, b]) => format!("> zoom({}, {})\n{}\n{}", node.start(), node.len(), clip(&state.bank.line(a), 140), clip(&state.bank.line(b), 140)),
    };
    let section = |id: u64, title: String, app: &mut PaneState| row_spaced(8., vec![mono(id!(id), title, 10, MUTE, app), column(vec![space(), square(id!(id)).fill(LINE).build(app).height(1.), space()])]).height(14.);
    Some(match i {
        0 => row_spaced(
            10.,
            vec![
                text(id!(), node.to_string()).font_family(DISPLAY).font_weight(FontWeight::EXTRA_BOLD).font_size(54).fill(AMBER).build(app),
                column(vec![space(), stack(vec![square(id!()).fill(color).build(app), display(id!(), format!("L{}", node.0), 15, BG, app)]).width(34.).height(22.).pad_bottom(6.)]),
                space(),
            ],
        )
        .height(60.)
        .pad_y(10.)
        .shadow((0., 0.), 18., AMBER.with_alpha(0.25))
        .height(80.),
        1 => column_spaced(
            6.,
            vec![
                kv(1, "COVERS", value(1, if leaf { format!("1 message · #{}", node.start()) } else { format!("{} messages · #{}–{}", node.len(), node.start(), node.end() - 1) }, app), app),
                kv(2, "TIME", value(2, if leaf { moment(&first.date) } else { format!("{} → {}", moment(&first.date), moment(&last.date)) }, app), app),
                kv(
                    3,
                    "SIZE",
                    column_spaced(4., vec![value(3, format!("{bytes} / {LIMIT} B{}", if verbatim { " · fits, no model call" } else { "" }), app), row(vec![gauge, space()])]),
                    app,
                ),
                kv(4, "PARENT", value(4, if parent.end() <= state.total() { format!("{parent} (L{})", parent.0) } else { "— none yet, still accumulating".to_string() }, app), app),
                kv(5, "IN VIEW", value(5, if node == line { "yes, as its own line".to_string() } else { format!("folded inside {line}") }, app), app),
            ],
        ),
        2 => column_spaced(
            8.,
            vec![
                section(6, if leaf { format!("VERBATIM · {}", first.kind.to_string().to_uppercase()) } else { "SUMMARY".to_string() }, app),
                row_spaced(
                    10.,
                    vec![
                        if leaf { square(id!()).fill(color).build(app).width(2.) } else { empty() },
                        text(id!(), if leaf { first.text.as_str() } else { summary.unwrap_or("not summarized yet") }).font_size(14).fill(TEXT).align(Alignment::Start).wrap().build(app).expand_x(),
                    ],
                ),
            ],
        ),
        3 => column_spaced(
            8.,
            vec![
                section(7, "AGENT TOOL CALL".to_string(), app),
                stack(vec![
                    square(id!()).fill(BG).stroke(LINE, Stroke::new(1.)).build(app),
                    text(id!(), zoom).font_family(MONO).font_size(12).fill(MUTE).align(Alignment::Start).wrap().build(app).expand_x().pad(10.),
                ]),
            ],
        ),
        _ => return None,
    }
    .pad_bottom(14.))
}

fn view<'a>(state: &'a State, app: &mut PaneState) -> View<'a, State> {
    let lines = state.lines.len();
    let view_bytes: usize = state.lines.iter().map(|&n| state.bank.line(n).len() + 1).sum();
    let depth = state.path.len() - 1;
    let searching = !state.query.text.trim().is_empty();
    let main = row_spaced(
        10.,
        vec![
            panel(
                1,
                "View",
                "視野",
                match searching {
                    true => format!("{} hits", state.hits.len()),
                    false => format!("{lines} nodes · {view_bytes} B"),
                },
                column(vec![
                    text_field(id!(), binding!(state.query))
                        .background(|query, _, app| square(id!()).fill(BG).stroke(if query.editing { AMBER } else { LINE_HI }, Stroke::new(1.)).build(app))
                        .hint_text("search memory · typos ok")
                        .font_family(MONO)
                        .font_size(12)
                        .text_fill(TEXT)
                        .hint_fill(FAINT)
                        .cursor_fill(AMBER)
                        .highlight_fill(AMBER_DIM)
                        .align(Alignment::Start)
                        .singleline()
                        .esc_end_editing()
                        .enter_end_editing()
                        .on_edit(|state: &mut State, app, edit| {
                            if let EditInteraction::Update(query) = edit {
                                state.find(&query);
                                app.redraw();
                            }
                        })
                        .build(app)
                        .height(30.)
                        .pad(8.),
                    match searching {
                        true => scroller(id!(state.hits.len() as u64), None, move |i, _, app| state.hits.get(i).map(|(n, body)| view_row(*n, body, state, app)), app),
                        false => scroller(id!(lines as u64), None, move |i, _, app| state.lines.get(i).map(|&n| view_row(n, state.bank.text(n).unwrap_or("not summarized yet"), state, app)), app),
                    },
                ]),
                app,
            )
            .width(300.),
            panel(
                2,
                "Zoom ladder",
                "降下",
                "each row = one zoom()".to_string(),
                column(vec![draw(move |area, ctx: &mut PaneState| crumbs(state, area.width, ctx).draw(area, ctx)).height(35.), draw(move |area, ctx: &mut PaneState| ladder(state, area, ctx).draw(area, ctx)).expand()]),
                app,
            )
            .expand_x(),
            panel(3, "Readout", "解析", format!("depth {depth} / {}", state.path[0].0), readout(state, app).pad_x(14.).pad_top(12.), app).width(340.),
        ],
    );
    let hints: Vec<View<'a, State>> = [("hover", "descend"), ("click", "lock / unlock"), ("↑↓", "level"), ("←→", "sibling"), ("esc", "unlock")]
        .into_iter()
        .enumerate()
        .map(|(k, (key, what))| {
            row_spaced(
                4.,
                vec![
                    stack(vec![square(id!(k as u64)).stroke(AMBER_DIM, Stroke::new(1.)).build(app), mono(id!(k as u64), key.to_uppercase(), 10, AMBER, app)]).width(key.chars().count() as f32 * 6.2 + 10.).height(15.),
                    mono(id!(k as u64), what.to_uppercase(), 10, MUTE, app),
                ],
            )
            .width((key.chars().count() + what.len()) as f32 * 6.2 + 20.)
        })
        .collect();
    let mut footer = hints;
    footer.push(space());
    footer.push(mono(id!(), state.dir.display().to_string(), 10, FAINT, app));
    stack(vec![
        square(id!())
            .fill(BG)
            .view()
            .gesture(gesture::key(id!()).run(|state: &mut State, app, event| {
                if event.phase == KeyPhase::Pressed && !state.query.editing {
                    state.step(&event.key);
                    app.redraw();
                }
            }))
            .build(app),
        path(id!(), |a| {
            let mut p = BezPath::new();
            let (x0, y0, x1, y1) = (a.x as f64, a.y as f64, (a.x + a.width) as f64, (a.y + a.height) as f64);
            let mut x = x0;
            while x < x1 {
                p.move_to((x, y0));
                p.line_to((x, y1));
                x += 40.;
            }
            let mut y = y0;
            while y < y1 {
                p.move_to((x0, y));
                p.line_to((x1, y));
                y += 40.;
            }
            p
        })
        .stroke(LINE.with_alpha(0.35), Stroke::new(1.))
        .build(app),
        square(id!())
            .fill(|a: Area, _: &()| Gradient::new_radial(((a.x + a.width / 2.) as f64, (a.y - a.height / 10.) as f64), 720.).with_stops([AMBER.with_alpha(0.07), TRANSPARENT]).into())
            .build(app),
        column_spaced(10., vec![header(state, app), schematic_panel(state, app), main.expand(), row_spaced(16., footer).height(16.)]).pad_x(16.).pad_top(34.).pad_bottom(12.),
        path(id!(), |a| {
            let mut p = BezPath::new();
            let mut y = a.y as f64;
            while y < (a.y + a.height) as f64 {
                p.move_to((a.x as f64, y + 2.5));
                p.line_to(((a.x + a.width) as f64, y + 2.5));
                y += 3.;
            }
            p
        })
        .stroke(Color::WHITE.with_alpha(0.022), Stroke::new(1.))
        .build(app),
    ])
}

fn start(state: &mut State, app: &mut PaneState) {
    let waker = app.waker();
    state.watcher = notify::recommended_watcher(move |_| waker.wake()).ok();
    if let Some(watcher) = &mut state.watcher {
        watcher.watch(&state.dir, RecursiveMode::Recursive).ok();
    }
}

fn reload(state: &mut State, app: &mut PaneState) {
    let Ok(bank) = Bank::open(&state.dir) else { return };
    if bank.messages.is_empty() {
        return;
    }
    let focus = *state.path.last().expect("the path always has its line");
    (state.bank, state.lines, state.built) = load(bank);
    state.path = state.route(focus);
    let query = state.query.text.clone();
    state.find(&query);
    app.redraw();
}

fn main() {
    let Some(dir) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: membank-gui <bank>");
        process::exit(1)
    };
    let bank = match Bank::open(&dir) {
        Ok(bank) if !bank.messages.is_empty() => bank,
        Ok(_) => {
            eprintln!("membank-gui: {} holds no messages; import some first", dir.display());
            process::exit(1)
        }
        Err(e) => {
            eprintln!("membank-gui: {e}");
            process::exit(1)
        }
    };
    let (bank, lines, built) = load(bank);
    let path = vec![lines[0]];
    let state = State { dir, bank, lines, built, path, locked: false, tip: None, span: 0.0..f32::INFINITY, ladder_scroll: 0., query: TextState::new(""), hits: Vec::new(), watcher: None };
    WinitApp::new(state).pane(PaneBuilder::new("main", view).on_start(start).on_wake(reload).title("membank").inner_size(1500, 940)).run();
}
