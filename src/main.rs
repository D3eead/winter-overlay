use gtk4::{cairo, glib, prelude::*};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use rand::Rng;
use std::{
    cell::RefCell, collections::HashMap, f64::consts::TAU, io::{Read, Write},
    os::unix::net::UnixStream, rc::Rc, time::Duration,
};

// =====================================================================
//                              SETTINGS
// =====================================================================

// ---- Snowfall ----
// Flakes spawned per tick. More snow: 10-15. Less snow: 2-3.
// (If you lower FPS_MS to 16, halve this value, it is counted per tick.)
const SPAWN: u32 = 2;
// Hard cap on flakes on screen. Weak PC: 3000. Strong PC: 15000.
const MAX_FLAKES: usize = 3000;
// Falling speed range (px/s). Faster storm: 200..350. Slow, calm snow: 40..90.
const FALL_MIN: f64 = 40.;
const FALL_MAX: f64 = 180.;
// Flake radius range (px). Bigger flakes: 2.5..4.5. Fine dust: 1.0..1.8.
const R_MIN: f64 = 1.5;
const R_MAX: f64 = 3.0;

// ---- Snow piles on windows ----
// Max pile height on window tops (px). Big drifts: 50+. Thin layer: 10-15.
const MAXH: f64 = 9.;
// Max height difference between neighbouring columns.
// Lower (1.5-2) = flat pile, snow slides a lot. Higher (5-8) = steep, tall peaks.
const SLOPE: f64 = 2.;
// Slide speed, 0..1. Fast slide: 0.6-0.9. Slow, lazy slide: 0.1-0.25.
const SLIDE: f64 = 0.9;
// Slide iterations per tick. Instant slide: 5+. Gentle: 1.
const SLIDE_PASSES: usize = 5;
// Height (px) that stays on the window edge before snow falls off.
// 0 = edges always clean. 10+ = snow hangs over the edge.
const EDGE_KEEP: f64 = 2.;
// How much of the overhang drops per tick, 0..1. Fast drop: 0.8. Slow: 0.2.
const EDGE_DROP: f64 = 0.9;
// Pile growth per landed flake: centre column / neighbour columns.
// Faster growth: 6.0 / 3.0. Slower: 1.0 / 0.5.
const ADD_CENTER: f64 = 3.0;
const ADD_SIDE: f64 = 1.5;
// How far (px) a window must jump between polls to count as a "shake".
// Easier to shake snow off: 30. Harder: 120.
const SHAKE_DIST: f64 = 60.;

// ---- Snow on the bottom of the screen ----
// Max floor snow height (px). Deeper snow: 150. Shallow: 30.
const FLOOR_MAX: f64 = 80.;
// Melting speed (px per tick). Never melts: 0.0. Fast melt: 0.02-0.05.
const MELT: f64 = 0.006;
// Melting speed of snow on window tops (px per tick).
// Fast melt: 0.1. Slow: 0.02. No melting: 0.0.
const WIN_MELT: f64 = 0.04 ;

// ---- Snowplow ----
// Plow speed (px/s). Faster: 1500. Slower: 300.
const PLOW_SPEED: f64 = 800.;
// Plow size and left margin (px).
const PW: f64 = 110.;
const PH: f64 = 50.;
const HOME: f64 = 16.;

// ---- Performance ----
// Frame time in ms. 33 = ~30 FPS (light). 16 = ~60 FPS (smoother, heavier).
const FPS_MS: u64 = 33;
const DT: f64 = FPS_MS as f64 / 1000.;
// Poll windows every N ticks. Lower = windows tracked more precisely, higher CPU.
const POLL_EVERY: u32 = 3;

// =====================================================================

struct Fl { x: f64, y: f64, vx: f64, vy: f64, r: f64, free: bool }
struct Win { x: f64, y: f64, w: f64, pile: Vec<f64> }
#[derive(PartialEq)]
enum Plow { Idle, Out, Back }
struct St {
    flakes: Vec<Fl>,
    floor: Vec<f64>,
    wins: HashMap<String, Win>,
    plow: Plow,
    px: f64,
    tick: u32,
    last: (i32, i32),
}

// Talk to Hyprland directly through its socket (no process spawning).
fn hypr(cmd: &str) -> Option<serde_json::Value> {
    let rt = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let mut s = UnixStream::connect(format!("{rt}/hypr/{sig}/.socket.sock")).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(200))).ok()?;
    s.write_all(cmd.as_bytes()).ok()?;
    let mut b = Vec::new();
    s.read_to_end(&mut b).ok()?;
    serde_json::from_slice(&b).ok()
}

// Returns (address, x, y, width) of every mapped window on visible workspaces.
fn poll() -> Vec<(String, f64, f64, f64)> {
    let mut out = vec![];
    let (Some(m), Some(c)) = (hypr("j/monitors"), hypr("j/clients")) else { return out };
    let act: Vec<i64> = m.as_array().into_iter().flatten()
        .filter_map(|m| m["activeWorkspace"]["id"].as_i64()).collect();
    for w in c.as_array().into_iter().flatten() {
        if w["mapped"].as_bool() == Some(true)
            && act.contains(&w["workspace"]["id"].as_i64().unwrap_or(-99))
        {
            if let (Some(a), Some(x), Some(y), Some(ww)) =
                (w["address"].as_str(), w["at"][0].as_f64(), w["at"][1].as_f64(), w["size"][0].as_f64())
            {
                out.push((a.to_string(), x, y, ww));
            }
        }
    }
    out
}

// A flake lands: raise the pile in column c and its neighbours.
fn add(p: &mut [f64], c: usize, max: f64) {
    for d in [-1i32, 0, 1] {
        let j = c as i32 + d;
        if j >= 0 && (j as usize) < p.len() {
            let j = j as usize;
            p[j] = (p[j] + if d == 0 { ADD_CENTER } else { ADD_SIDE }).min(max);
        }
    }
}

// Pile physics: snow slides down slopes, tall spots get cut to MAXH,
// and the overhang at the window edges falls off as flakes.
fn relax(w: &mut Win, out: &mut Vec<Fl>, r: &mut impl Rng) {
    let n = w.pile.len();
    if n < 2 { return; }
    let p = &mut w.pile;
    for i in 0..n - 1 {
        let d = p[i] - p[i + 1];
        if d > SLOPE { let m = (d - SLOPE) * SLIDE; p[i] -= m; p[i + 1] += m; }
        else if d < -SLOPE { let m = (-d - SLOPE) * SLIDE; p[i] += m; p[i + 1] -= m; }
    }
    for i in 0..n {
        if p[i] > MAXH {
            let e = p[i] - MAXH;
            p[i] = MAXH;
            if i > 0 { p[i - 1] += e * 0.5; }
            if i + 1 < n { p[i + 1] += e * 0.5; }
        }
    }
    for (i, dir) in [(0usize, -1.0f64), (n - 1, 1.0)] {
        if p[i] > EDGE_KEEP {
            let d = (p[i] - EDGE_KEEP) * EDGE_DROP;
            p[i] -= d;
            for _ in 0..(d.ceil() as usize).min(1) {
    if !r.gen_bool(0.3) { continue; }
                out.push(Fl {
                    x: if dir < 0. { w.x } else { w.x + w.w },
                    y: w.y - p[i],
                    vx: dir * r.gen_range(10.0..50.0),
                    vy: r.gen_range(40.0..120.0),
                    r: r.gen_range(1.2..2.5),
                    free: true,
                });
            }
        }
    }
}

fn rrect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -TAU / 4., 0.);
    cr.arc(x + w - r, y + h - r, r, 0., TAU / 4.);
    cr.arc(x + r, y + h - r, r, TAU / 4., TAU / 2.);
    cr.arc(x + r, y + r, r, TAU / 2., TAU * 0.75);
    cr.close_path();
}

// Snowplow drawing. Change the cr.set_source_rgb(...) values to recolor it:
// body orange = (1.0, 0.55, 0.05), blade = (0.72, 0.75, 0.8).
fn draw_plow(cr: &cairo::Context, x: f64, y: f64, tick: u32, moving: bool) {
    cr.save().ok();
    cr.translate(x, y);
    // headlight beam while moving
    if moving {
        cr.set_source_rgba(1., 0.95, 0.5, 0.18);
        cr.move_to(88., 26.); cr.line_to(130., 14.); cr.line_to(130., 40.); cr.close_path(); cr.fill().ok();
    }
    // blade arm
    cr.set_source_rgb(0.25, 0.25, 0.3);
    cr.rectangle(84., 31., 16., 4.); cr.fill().ok();
    // blade with yellow stripe
    cr.set_source_rgb(0.72, 0.75, 0.8);
    cr.move_to(96., 12.); cr.line_to(108., 6.); cr.line_to(110., 44.); cr.line_to(98., 46.); cr.close_path(); cr.fill().ok();
    cr.set_source_rgb(1., 0.8, 0.1);
    cr.move_to(97., 20.); cr.line_to(109., 16.); cr.line_to(109.5, 24.); cr.line_to(97.5, 28.); cr.close_path(); cr.fill().ok();
    // body
    cr.set_source_rgb(1., 0.55, 0.05);
    rrect(cr, 0., 22., 90., 18., 4.); cr.fill().ok();
    cr.set_source_rgb(0.15, 0.15, 0.18);
    cr.rectangle(0., 34., 90., 3.); cr.fill().ok();
    // cabin
    cr.set_source_rgb(1., 0.55, 0.05);
    cr.move_to(20., 22.); cr.line_to(27., 5.); cr.line_to(58., 5.); cr.line_to(68., 22.); cr.close_path(); cr.fill().ok();
    // window
    cr.set_source_rgb(0.68, 0.85, 0.95);
    cr.move_to(26., 20.); cr.line_to(31., 8.); cr.line_to(55., 8.); cr.line_to(62., 20.); cr.close_path(); cr.fill().ok();
    cr.set_source_rgba(1., 1., 1., 0.5);
    cr.move_to(33., 8.); cr.line_to(39., 8.); cr.line_to(33., 20.); cr.line_to(29., 20.); cr.close_path(); cr.fill().ok();
    // flashing beacon (change 8 to blink faster/slower)
    let on = (tick / 8) % 2 == 0;
    if on { cr.set_source_rgba(1., 0.6, 0., 0.3); cr.arc(42., 2., 8., 0., TAU); cr.fill().ok(); }
    cr.set_source_rgb(if on { 1. } else { 0.55 }, if on { 0.7 } else { 0.3 }, 0.);
    rrect(cr, 37., 0., 10., 5., 2.); cr.fill().ok();
    // headlight and tail light
    cr.set_source_rgb(1., 0.95, 0.6);
    cr.arc(87., 28., 3., 0., TAU); cr.fill().ok();
    cr.set_source_rgb(0.85, 0.1, 0.1);
    cr.rectangle(0., 27., 3., 5.); cr.fill().ok();
    // wheels
    for cx in [22., 68.] {
        cr.set_source_rgb(0.08, 0.08, 0.1);
        cr.arc(cx, 40., 10., 0., TAU); cr.fill().ok();
        cr.set_source_rgb(0.6, 0.62, 0.68);
        cr.arc(cx, 40., 5., 0., TAU); cr.fill().ok();
        cr.set_source_rgb(0.3, 0.3, 0.35);
        cr.arc(cx, 40., 1.8, 0., TAU); cr.fill().ok();
    }
    cr.restore().ok();
}

fn main() {
    let app = gtk4::Application::builder().application_id("dev.snow").build();
    app.connect_activate(|app| {
        // Fullscreen transparent overlay above everything
        let win = gtk4::ApplicationWindow::new(app);
        win.init_layer_shell();
        win.set_layer(Layer::Overlay);
        win.set_exclusive_zone(-1);
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] { win.set_anchor(e, true); }
        let css = gtk4::CssProvider::new();
        css.load_from_data("window{background:transparent}");
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().unwrap(), &css, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let area = gtk4::DrawingArea::new();
        win.set_child(Some(&area));
        let st = Rc::new(RefCell::new(St {
            flakes: vec![], floor: vec![], wins: HashMap::new(),
            plow: Plow::Idle, px: HOME, tick: 0, last: (-1, -1),
        }));

        // Click on the plow: start cleaning
        let s3 = st.clone();
        let click = gtk4::GestureClick::new();
        click.connect_pressed(move |_, _, _, _| {
            let mut s = s3.borrow_mut();
            if s.plow == Plow::Idle { s.plow = Plow::Out; }
        });
        area.add_controller(click);

        // Drawing
        let s2 = st.clone();
        area.set_draw_func(move |_, cr, _, dh| {
            let s = s2.borrow();
            let dh = dh as f64;
            // Snow color / opacity: (r, g, b, alpha)
            cr.set_source_rgba(1., 1., 1., 0.95);
            for (i, p) in s.floor.iter().enumerate() {
                if *p > 0.3 { cr.rectangle(i as f64 * 4., dh - p, 4., *p); }
            }
            for w in s.wins.values() {
                for (i, p) in w.pile.iter().enumerate() {
                    if *p > 0.3 { cr.rectangle(w.x + i as f64 * 4., w.y - p, 4., *p); }
                }
            }
            cr.fill().ok();
            for f in &s.flakes { cr.new_sub_path(); cr.arc(f.x, f.y, f.r, 0., TAU); }
            cr.fill().ok();
            draw_plow(cr, s.px, dh - PH - 2., s.tick, s.plow != Plow::Idle);
        });

        // Main loop
        let a2 = area.clone();
        let win2 = win.clone();
        glib::timeout_add_local(Duration::from_millis(FPS_MS), move || {
            let (aw, h) = (a2.width() as f64, a2.height() as f64);
            if aw < 1. { return glib::ControlFlow::Continue; }
            let mut g = st.borrow_mut();
            let s = &mut *g;
            let mut r = rand::thread_rng();

            let cols = (aw / 4.) as usize;
            if s.floor.len() != cols { s.floor.resize(cols, 0.); }
            s.tick += 1;

            // Poll windows, detect shaking
            if s.tick % POLL_EVERY == 1 {
                let mut nw = HashMap::new();
                for (id, x, y, w) in poll() {
                    let n = (w / 4.) as usize;
                    let mut pile = vec![0.; n];
                    if let Some(o) = s.wins.get(&id) {
                        pile = o.pile.clone();
                        pile.resize(n, 0.);
                        if ((x - o.x).powi(2) + (y - o.y).powi(2)).sqrt() > SHAKE_DIST {
                            for (i, p) in pile.iter().enumerate() {
                                if *p > 0.5 {
                                    s.flakes.push(Fl {
                                        x: x + i as f64 * 4., y: y - p,
                                        vx: r.gen_range(-40.0..40.0), vy: r.gen_range(80.0..200.0),
                                        r: r.gen_range(1.0..2.5), free: true,
                                    });
                                }
                            }
                            pile.iter_mut().for_each(|p| *p = 0.);
                        }
                    }
                    nw.insert(id, Win { x, y, w, pile });
                }
                s.wins = nw;
            }

            // Piles slide / fall off the edges
            for w in s.wins.values_mut() {
                for _ in 0..SLIDE_PASSES { relax(w, &mut s.flakes, &mut r); }
            }
for w in s.wins.values_mut() {
    for v in w.pile.iter_mut() { if *v > 0. { *v = (*v - WIN_MELT).max(0.); } }
}
            // New snow
            if s.flakes.len() < MAX_FLAKES {
                for _ in 0..SPAWN {
                    s.flakes.push(Fl {
                        x: r.gen_range(0.0..aw), y: -5., vx: 0.,
                        vy: r.gen_range(FALL_MIN..FALL_MAX), r: r.gen_range(R_MIN..R_MAX), free: false,
                    });
                }
            }

            // Flake physics
            {
                let wins = &mut s.wins;
                let floor = &mut s.floor;
                s.flakes.retain_mut(|f| {
                    let py = f.y;
                    f.y += f.vy * DT;
                    f.x += f.vx * DT + if f.free { 0. } else { (f.y * 0.05).sin() * 0.3 };
                    if !f.free {
                        for w in wins.values_mut() {
                            if f.x >= w.x && f.x < w.x + w.w {
                                let c = ((f.x - w.x) / 4.) as usize;
                                if c < w.pile.len() {
                                    let top = w.y - w.pile[c];
                                    if py <= top && f.y >= top {
                                        add(&mut w.pile, c, MAXH + 10.);
                                        return false;
                                    }
                                }
                            }
                        }
                    }
                    if f.x >= 0. {
                        let c = (f.x / 4.) as usize;
                        if c < floor.len() && f.y >= h - floor[c] {
                            add(floor, c, FLOOR_MAX);
                            return false;
                        }
                    }
                    f.y < h + 50.
                });
            }

            // Floor snow slowly melts
            for v in s.floor.iter_mut() { if *v > 0. { *v = (*v - MELT).max(0.); } }

            // Plow movement
            match s.plow {
                Plow::Out => {
                    s.px += PLOW_SPEED * DT;
                    let front = s.px + PW;
                    for (c, v) in s.floor.iter_mut().enumerate() {
                        if (c as f64 * 4.) < front { *v = 0.; }
                    }
                    if s.px >= aw - PW - HOME { s.plow = Plow::Back; }
                }
                Plow::Back => {
                    s.px -= PLOW_SPEED * DT;
                    if s.px <= HOME { s.px = HOME; s.plow = Plow::Idle; }
                }
                Plow::Idle => {}
            }

            // Only the plow is clickable, everything else is click-through
            let key = (s.px as i32, h as i32);
            if key != s.last {
                if let Some(sf) = win2.surface() {
                    sf.set_input_region(&cairo::Region::create_rectangle(
                        &cairo::RectangleInt::new(s.px as i32, (h - PH - 2.) as i32, PW as i32, PH as i32)));
                    s.last = key;
                }
            }

            a2.queue_draw();
            glib::ControlFlow::Continue
        });
        win.present();
    });
    app.run();
}