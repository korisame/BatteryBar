#![allow(deprecated, unexpected_cfgs)]
use crate::{
    activity_table,
    history::Report,
    model::{self, Power, Store},
    platform,
};
use cocoa::{
    appkit::{NSApp, NSApplication, NSApplicationActivationPolicyAccessory, NSColor, NSImage},
    base::{id, nil, NO, YES},
    foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString},
};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Class, Object, Sel},
    sel, sel_impl,
};
use std::{cell::RefCell, collections::HashMap, path::PathBuf};

thread_local! {static APP:RefCell<Option<App>>=const{RefCell::new(None)};}
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}
unsafe fn string(s: &str) -> id {
    NSString::alloc(nil).init_str(s).autorelease()
}
unsafe fn message_text(label: id, text: &str) {
    let _: () = msg_send![label,setStringValue:string(text)];
}
unsafe fn color(r: f64, g: f64, b: f64, a: f64) -> id {
    NSColor::colorWithSRGBRed_green_blue_alpha_(nil, r, g, b, a)
}
fn connected_draining(s: &model::Snapshot) -> bool {
    s.state != Power::Discharging
        && (s.insufficient || (s.state == Power::Plugged && s.watts().is_some_and(|w| w < -1.)))
}
fn visible_power_state(s: &model::Snapshot) -> &'static str {
    if s.stale || !s.has_reading {
        "Stato batteria non disponibile"
    } else if connected_draining(s) {
        "Alimentatore collegato · batteria in scarica"
    } else if s.state == Power::Charging {
        "Batteria in carica"
    } else {
        s.state.label()
    }
}
fn estimate_heading(s: &model::Snapshot) -> &'static str {
    if connected_draining(s) || s.state == Power::Discharging {
        "TEMPO ALLA SCARICA"
    } else if s.state == Power::Charging {
        "TEMPO ALLA CARICA COMPLETA"
    } else if s.state == Power::Charged {
        "CARICA COMPLETA"
    } else {
        "STIMA NON DISPONIBILE"
    }
}
fn power_symbol(s: &model::Snapshot) -> &'static str {
    if connected_draining(s) {
        "arrow.down.circle.fill"
    } else {
        match s.state {
            Power::Charging => "bolt.fill",
            Power::Discharging => "battery.100",
            Power::Charged => "checkmark.circle.fill",
            Power::Plugged => "powerplug.fill",
        }
    }
}
fn tint(s: &model::Snapshot) -> (f64, f64, f64) {
    if connected_draining(s) {
        return (0.94, 0.58, 0.21);
    }
    match s.state {
        Power::Charged => (0.18, 0.80, 0.66),
        Power::Charging | Power::Plugged => (0.52, 0.60, 0.98),
        Power::Discharging => match s.percent {
            0..=9 => (0.90, 0.28, 0.30),
            10..=24 => (0.85, 0.47, 0.34),
            25..=44 => (0.96, 0.72, 0.31),
            _ => (0.18, 0.80, 0.66),
        },
    }
}
unsafe fn set_color(c: id) {
    let _: () = msg_send![c, set];
}
unsafe fn rounded(r: NSRect, radius: f64, fill: id) {
    set_color(fill);
    let p: id =
        msg_send![class!(NSBezierPath),bezierPathWithRoundedRect:r xRadius:radius yRadius:radius];
    let _: () = msg_send![p, fill];
}
unsafe fn card(parent: id, r: NSRect, radius: f64) {
    let view: id = msg_send![class!(NSBox), alloc];
    let view: id = msg_send![view,initWithFrame:r];
    let _: () = msg_send![view,setBoxType:4_usize];
    let _: () = msg_send![view,setTitlePosition:0_usize];
    let _: () = msg_send![view,setBorderType:0_usize];
    let _: () = msg_send![view,setCornerRadius:radius];
    let base: id = msg_send![class!(NSColor), labelColor];
    let fill: id = msg_send![base,colorWithAlphaComponent:0.055_f64];
    let _: () = msg_send![view,setFillColor:fill];
    let _: () = msg_send![parent,addSubview:view];
    let _: () = msg_send![view, release];
}
unsafe fn metric_icon(parent: id, name: &str, r: NSRect, tint: (f64, f64, f64)) {
    let image: id = msg_send![class!(NSImage),imageWithSystemSymbolName:string(name) accessibilityDescription:nil];
    let view: id = msg_send![class!(NSImageView), alloc];
    let view: id = msg_send![view,initWithFrame:r];
    let _: () = msg_send![view,setImage:image];
    let _: () = msg_send![view,setContentTintColor:color(tint.0,tint.1,tint.2,1.)];
    let _: () = msg_send![parent,addSubview:view];
    let _: () = msg_send![view, release];
}
unsafe fn font(size: f64, bold: bool) -> id {
    msg_send![class!(NSFont),systemFontOfSize:size weight:if bold{0.23_f64}else{0.0_f64}]
}
unsafe fn label(parent: id, text: &str, r: NSRect, size: f64, bold: bool, secondary: bool) -> id {
    let l: id = msg_send![class!(NSTextField),labelWithString:string(text)];
    let _: () = msg_send![l,setFrame:r];
    let _: () = msg_send![l,setFont:font(size,bold)];
    if secondary {
        let c: id = msg_send![class!(NSColor), secondaryLabelColor];
        let _: () = msg_send![l,setTextColor:c];
    }
    let _: () = msg_send![l,setLineBreakMode:4_usize];
    let _: () = msg_send![l,setToolTip:string(text)];
    let _: () = msg_send![parent,addSubview:l];
    l
}
unsafe fn multiline(parent: id, text: &str, x: f64, y: f64, w: f64, h: f64, size: f64) -> id {
    let l = label(parent, text, rect(x, y, w, h), size, false, true);
    let _: () = msg_send![l,setLineBreakMode:0_usize];
    let _: () = msg_send![l,setMaximumNumberOfLines:0_isize];
    l
}
unsafe fn button(parent: id, title: &str, r: NSRect, target: id, tag: i64) -> id {
    let b: id = msg_send![class!(BatteryBarRustButton), alloc];
    let b: id = msg_send![b,initWithFrame:r];
    let _: () = msg_send![b,setTitle:string(title)];
    let _: () = msg_send![b,setTarget:target];
    let _: () = msg_send![b,setAction:sel!(action:)];
    let _: () = msg_send![b,setTag:tag];
    let _: () = msg_send![b,setFont:font(11.,false)];
    let _: () = msg_send![b,setBezelStyle:1_usize];
    let _: () = msg_send![parent,addSubview:b];
    let _: () = msg_send![b, release];
    b
}
unsafe fn toggle(parent: id, title: &str, y: f64, on: bool, target: id, tag: i64) {
    label(
        parent,
        title,
        rect(16., y + 1., 280., 22.),
        11.,
        false,
        false,
    );
    let b: id = msg_send![class!(NSSwitch), alloc];
    let b: id = msg_send![b,initWithFrame:rect(308.,y,42.,24.)];
    let _: () = msg_send![b,setState:if on{1_isize}else{0_isize}];
    let _: () = msg_send![b,setTarget:target];
    let _: () = msg_send![b,setAction:sel!(action:)];
    let _: () = msg_send![b,setTag:tag];
    let _: () = msg_send![parent,addSubview:b];
    let _: () = msg_send![b, release];
}
unsafe fn segments(parent: id, titles: &[String], selected: usize, y: f64, target: id, tag: i64) {
    let b: id = msg_send![class!(BatteryBarRustSegments), alloc];
    let b: id = msg_send![b,initWithFrame:rect(16.,y,348.,26.)];
    let _: () = msg_send![b,setSegmentCount:titles.len() as isize];
    for (i, title) in titles.iter().enumerate() {
        let _: () = msg_send![b,setLabel:string(title) forSegment:i as isize];
        let _: () = msg_send![b,setWidth:348.0/titles.len() as f64 forSegment:i as isize];
    }
    let _: () = msg_send![b,setSelectedSegment:selected as isize];
    let _: () = msg_send![b,setTarget:target];
    let _: () = msg_send![b,setAction:sel!(action:)];
    let _: () = msg_send![b,setTag:tag];
    let _: () = msg_send![parent,addSubview:b];
    let _: () = msg_send![b, release];
}
unsafe fn section(parent: id, title: &str, y: f64) {
    label(parent, title, rect(16., y, 348., 16.), 9., true, true);
}
unsafe fn clock(t: f64) -> String {
    let seconds = t as libc::time_t;
    let mut tm: libc::tm = std::mem::zeroed();
    libc::localtime_r(&seconds, &mut tm);
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}
struct Panel {
    controller: id,
    root: id,
    labels: HashMap<&'static str, id>,
    gauge: id,
    chart: id,
    table: id,
    consumers: [ConsumerViews; 6],
    activity_rows: usize,
}
#[derive(Clone, Copy)]
struct ConsumerViews {
    name: id,
    cpu: id,
    watts: id,
    progress: id,
}
impl Drop for Panel {
    fn drop(&mut self) {
        unsafe {
            if self.table != nil {
                activity_table::detach(self.table);
                let _: () = msg_send![self.table, release];
            }
            let _: () = msg_send![self.controller, release];
        }
    }
}
pub struct Options {
    pub isolated: bool,
    pub show_panel: bool,
    pub quit_after: Option<f64>,
    pub capture: Option<PathBuf>,
    pub settings_open: bool,
    pub close_after: Option<f64>,
    pub panel_cycles: u32,
    pub exercise_ui: bool,
    pub hold_panel: bool,
    pub regression_scroll: bool,
    pub capture_viewport: bool,
    pub expanded_cycles: bool,
    pub memory_every: f64,
}
type PanelMode = (Power, bool, bool, Option<i64>, Option<i64>, bool, bool);
struct App {
    store: Store,
    status: id,
    popover: id,
    target: id,
    panel: Option<Panel>,
    report: Report,
    show_settings: bool,
    show_all: bool,
    last_collect: f64,
    settle_until: f64,
    last_settle: f64,
    start: f64,
    options: Options,
    bar_text: String,
    bar_image_key: (i64, Power, bool, bool, bool),
    panel_mode: Option<PanelMode>,
    memory_samples: Vec<serde_json::Value>,
    last_memory: f64,
    cycle_step: u32,
    ui_checks: Vec<serde_json::Value>,
    pending_rebuild: bool,
    pending_report: bool,
    regression: Option<Regression>,
    lifecycle: Vec<serde_json::Value>,
    sweep_step: usize,
    not_before_quit: f64,
}
struct Regression {
    offset: f64,
    period: i64,
    group: bool,
    settings: bool,
    expanded: bool,
    focus: i64,
    collected: f64,
    checked: bool,
}
unsafe fn responder_tag(window: id) -> i64 {
    if window == nil {
        return 0;
    }
    let r: id = msg_send![window, firstResponder];
    if r == nil {
        return 0;
    }
    let supports: bool = msg_send![r,respondsToSelector:sel!(tag)];
    if supports {
        msg_send![r, tag]
    } else {
        0
    }
}
unsafe fn child_tag(root: id, tag: i64) -> id {
    let children: id = msg_send![root, subviews];
    let count: usize = msg_send![children, count];
    for i in 0..count {
        let c: id = msg_send![children,objectAtIndex:i];
        let found: i64 = msg_send![c, tag];
        if found == tag {
            return c;
        }
    }
    nil
}
unsafe fn register() -> *const Class {
    activity_table::register();
    let mut target = ClassDecl::new("BatteryBarRustController", class!(NSObject)).unwrap();
    target.add_method(sel!(tick:), tick as extern "C" fn(&Object, Sel, id));
    target.add_method(sel!(action:), action as extern "C" fn(&Object, Sel, id));
    target.add_method(
        sel!(togglePanel:),
        toggle_panel as extern "C" fn(&Object, Sel, id),
    );
    let class = target.register();
    let mut flipped = ClassDecl::new("BatteryBarRustFlipped", class!(NSView)).unwrap();
    flipped.add_method(
        sel!(isFlipped),
        is_flipped as extern "C" fn(&Object, Sel) -> bool,
    );
    flipped.add_method(
        sel!(wantsUpdateLayer),
        is_flipped as extern "C" fn(&Object, Sel) -> bool,
    );
    flipped.add_method(
        sel!(updateLayer),
        empty_layer as extern "C" fn(&Object, Sel),
    );
    flipped.register();
    let mut button = ClassDecl::new("BatteryBarRustButton", class!(NSButton)).unwrap();
    button.add_method(
        sel!(drawRect:),
        draw_button as extern "C" fn(&Object, Sel, NSRect),
    );
    button.register();
    let mut segments =
        ClassDecl::new("BatteryBarRustSegments", class!(NSSegmentedControl)).unwrap();
    segments.add_method(
        sel!(drawRect:),
        draw_segments as extern "C" fn(&Object, Sel, NSRect),
    );
    segments.register();
    let mut gauge = ClassDecl::new("BatteryBarRustGauge", class!(NSView)).unwrap();
    gauge.add_method(
        sel!(drawRect:),
        draw_gauge as extern "C" fn(&Object, Sel, NSRect),
    );
    gauge.register();
    let mut chart = ClassDecl::new("BatteryBarRustChart", class!(NSView)).unwrap();
    chart.add_method(
        sel!(drawRect:),
        draw_chart as extern "C" fn(&Object, Sel, NSRect),
    );
    chart.register();
    class
}
extern "C" fn is_flipped(_: &Object, _: Sel) -> bool {
    YES
}
extern "C" fn empty_layer(_: &Object, _: Sel) {}
unsafe fn draw_text(text: id, r: NSRect, size: f64, foreground: id) {
    let objects = [font(size, false), foreground];
    let keys = [string("NSFont"), string("NSColor")];
    let attrs: id = msg_send![class!(NSDictionary),dictionaryWithObjects:objects.as_ptr() forKeys:keys.as_ptr() count:2_usize];
    let measured: NSSize = msg_send![text,sizeWithAttributes:attrs];
    let area = rect(
        r.origin.x + (r.size.width - measured.width) / 2.,
        r.origin.y + (r.size.height - measured.height) / 2.,
        measured.width,
        measured.height,
    );
    let _: () = msg_send![text,drawInRect:area withAttributes:attrs];
}
extern "C" fn draw_button(this: &Object, _: Sel, _: NSRect) {
    unsafe {
        let bounds: NSRect = msg_send![this, bounds];
        let c: id = msg_send![class!(NSColor), labelColor];
        let bg: id = msg_send![c,colorWithAlphaComponent:0.10_f64];
        rounded(bounds, 7., bg);
        let title: id = msg_send![this, title];
        draw_text(title, bounds, 11., c);
    }
}
extern "C" fn draw_segments(this: &Object, _: Sel, _: NSRect) {
    unsafe {
        let bounds: NSRect = msg_send![this, bounds];
        let c: id = msg_send![class!(NSColor), labelColor];
        let bg: id = msg_send![c,colorWithAlphaComponent:0.07_f64];
        rounded(bounds, 6., bg);
        let n: isize = msg_send![this, segmentCount];
        let selected: isize = msg_send![this, selectedSegment];
        let width = bounds.size.width / n.max(1) as f64;
        for i in 0..n {
            let r = rect(
                i as f64 * width + 2.,
                2.,
                width - 4.,
                bounds.size.height - 4.,
            );
            let foreground = if i == selected {
                rounded(r, 5., color(0.32, 0.48, 0.91, 1.));
                color(1., 1., 1., 1.)
            } else {
                c
            };
            let title: id = msg_send![this,labelForSegment:i];
            draw_text(title, r, 11., foreground);
        }
    }
}
extern "C" fn tick(_: &Object, _: Sel, _: id) {
    let exercise = APP.with(|cell| {
        let mut b = cell.borrow_mut();
        if let Some(a) = b.as_mut() {
            if a.options.exercise_ui && model::now() - a.start >= 2. {
                a.options.exercise_ui = false;
                return true;
            }
        }
        false
    });
    if exercise {
        let checks = unsafe { exercise_ui() };
        APP.with(|cell| {
            let mut b = cell.borrow_mut();
            let a = b.as_mut().unwrap();
            a.ui_checks = checks;
            a.not_before_quit = model::now() + 3.;
        });
    }
    let quit = APP.with(|cell| {
        cell.borrow().as_ref().is_some_and(|a| {
            model::now() - a.start >= a.options.quit_after.unwrap_or(f64::INFINITY)
                && model::now() >= a.not_before_quit
        })
    });
    if quit {
        APP.with(|cell| {
            let b = cell.borrow();
            if let Some(a) = b.as_ref() {
                let memory_before_capture=platform::memory();
                unsafe {
                    if let (Some(path), Some(p)) = (a.options.capture.as_ref(), a.panel.as_ref()) {
                        let view=if a.options.capture_viewport{msg_send![p.controller,view]}else{p.root};
                        let _ = platform::capture(view.cast(),path.to_string_lossy().as_ref());
                    }
                }
                println!("{}",serde_json::json!({"memory":memory_before_capture,"memory_samples":a.memory_samples,"memory_after_capture":platform::memory(),"history_counts":a.store.db.counts().ok(),"snapshot":a.store.snapshot,"seconds":model::now()-a.start,"panel_open":a.panel.is_some(),"ui_checks":a.ui_checks,"activity_table":activity_table::diagnostics(),"lifecycle":a.lifecycle}));
            }
        });
        platform::stop_events();
        unsafe {
            let _: () = msg_send![NSApp(),terminate:nil];
        }
        return;
    }
    APP.with(|cell| {
        if let Ok(mut b) = cell.try_borrow_mut() {
            if let Some(a) = b.as_mut() {
                unsafe {
                    a.tick();
                }
            }
        }
    });
}
extern "C" fn action(_: &Object, _: Sel, sender: id) {
    APP.with(|cell| {
        if let Ok(mut b) = cell.try_borrow_mut() {
            if let Some(a) = b.as_mut() {
                unsafe {
                    a.action(sender);
                }
            }
        }
    });
}
extern "C" fn toggle_panel(_: &Object, _: Sel, _: id) {
    unsafe {
        let event: id = msg_send![NSApp(), currentEvent];
        let kind: usize = if event != nil {
            msg_send![event, type]
        } else {
            0
        };
        if kind == 4 {
            let (status, target) = APP.with(|cell| {
                let b = cell.borrow();
                let a = b.as_ref().unwrap();
                (a.status, a.target)
            });
            context_menu(status, target);
            return;
        }
    }
    APP.with(|cell| {
        if let Ok(mut b) = cell.try_borrow_mut() {
            if let Some(a) = b.as_mut() {
                unsafe {
                    let shown: bool = msg_send![a.popover, isShown];
                    if a.options.hold_panel && shown {
                        a.trace("held_status_toggle_ignored");
                        return;
                    }
                    a.toggle();
                }
            }
        }
    });
}
extern "C" fn event(kind: i32) {
    APP.with(|cell| {
        if let Ok(mut b) = cell.try_borrow_mut() {
            if let Some(a) = b.as_mut() {
                unsafe {
                    match kind {
                        1 | 2 | 4 => {
                            if kind == 2 {
                                a.store.estimator.reset();
                            }
                            a.store.refresh();
                            a.settle_until = model::now() + 2.;
                            a.update();
                        }
                        3 => {
                            if let Some(p) = a.panel.as_ref() {
                                let window: id = msg_send![p.root, window];
                                if window != nil {
                                    let frame: NSRect = msg_send![window, frame];
                                    let point: NSPoint = msg_send![class!(NSEvent), mouseLocation];
                                    if point.x >= frame.origin.x
                                        && point.x <= frame.origin.x + frame.size.width
                                        && point.y >= frame.origin.y
                                        && point.y <= frame.origin.y + frame.size.height
                                    {
                                        return;
                                    }
                                }
                            }
                            a.close("watch");
                        }
                        _ => {}
                    }
                }
            }
        }
    });
}
extern "C" fn draw_gauge(_: &Object, _: Sel, _: NSRect) {
    APP.with(|cell|{if let Ok(b)=cell.try_borrow(){if let Some(a)=b.as_ref(){unsafe{
 let s=&a.store.snapshot;let(r,g,b)=tint(s);let p:id=msg_send![class!(NSBezierPath),bezierPath];let _:()=msg_send![p,appendBezierPathWithOvalInRect:rect(8.,8.,72.,72.)];let _:()=msg_send![p,setLineWidth:7.0_f64];set_color(color(r,g,b,0.15));let _:()=msg_send![p,stroke];
 let p:id=msg_send![class!(NSBezierPath),bezierPath];let _:()=msg_send![p,appendBezierPathWithArcWithCenter:NSPoint::new(44.,44.) radius:36.0_f64 startAngle:90.0_f64 endAngle:(90.0-360.0*s.percent as f64/100.0) clockwise:YES];let _:()=msg_send![p,setLineWidth:7.0_f64];let _:()=msg_send![p,setLineCapStyle:1_usize];set_color(color(r,g,b,1.));let _:()=msg_send![p,stroke];
 // The native progress indicator provides the original charging animation without a resident UI tree.
}}}});
}
extern "C" fn draw_chart(this: &Object, _: Sel, _: NSRect) {
    APP.with(|cell| {
        if let Ok(b) = cell.try_borrow() {
            if let Some(a) = b.as_ref() {
                unsafe {
                    let bounds: NSRect = msg_send![this, bounds];
                    let w = bounds.size.width - 28.;
                    let h = bounds.size.height - 22.;
                    let t = model::now();
                    let hours = a.store.settings.int("activity_history_hours") as f64;
                    let since = t - hours * 3600.;
                    let grid: id = msg_send![class!(NSBezierPath), bezierPath];
                    for pct in [0., 50., 100.] {
                        let y = 10. + h * pct / 100.;
                        let _: () = msg_send![grid,moveToPoint:NSPoint::new(0.,y)];
                        let _: () = msg_send![grid,lineToPoint:NSPoint::new(w,y)];
                    }
                    let _: () = msg_send![grid,setLineWidth:0.5_f64];
                    set_color(color(0.5, 0.5, 0.5, 0.25));
                    let _: () = msg_send![grid, stroke];
                    let line: id = msg_send![class!(NSBezierPath), bezierPath];
                    let area: id = msg_send![class!(NSBezierPath), bezierPath];
                    let mut seen = std::collections::HashSet::new();
                    let mut first = true;
                    let mut last_x = 0.;
                    for p in a.store.history.iter().filter(|p| p.ts >= since) {
                        if !seen.insert((p.ts / 60.) as i64) {
                            continue;
                        }
                        let point = NSPoint::new(
                            ((p.ts - since) / (t - since) * w).clamp(0., w),
                            10. + h * p.pct as f64 / 100.,
                        );
                        if first {
                            let _: () = msg_send![line,moveToPoint:point];
                            let _: () = msg_send![area,moveToPoint:NSPoint::new(point.x,10.)];
                            let _: () = msg_send![area,lineToPoint:point];
                            first = false;
                        } else {
                            let _: () = msg_send![line,lineToPoint:point];
                            let _: () = msg_send![area,lineToPoint:point];
                        }
                        last_x = point.x;
                    }
                    if !first {
                        let _: () = msg_send![area,lineToPoint:NSPoint::new(last_x,10.)];
                        let _: () = msg_send![area, closePath];
                        let (r, g, b) = tint(&a.store.snapshot);
                        set_color(color(r, g, b, 0.14));
                        let _: () = msg_send![area, fill];
                        set_color(color(r, g, b, 1.));
                        let _: () = msg_send![line,setLineWidth:1.7_f64];
                        let _: () = msg_send![line, stroke];
                    }
                }
            }
        }
    });
}
impl App {
    unsafe fn trace(&mut self, event: &str) {
        if self.options.isolated && self.lifecycle.len() < 256 {
            let shown: bool = msg_send![self.popover, isShown];
            self.lifecycle.push(serde_json::json!({"seconds":model::now()-self.start,"event":event,"shown":shown,"expanded":self.show_all,"tables":activity_table::diagnostics()}));
        }
    }
    unsafe fn update(&mut self) {
        let s = &self.store.snapshot;
        let mode = (
            s.state,
            s.has_reading,
            s.insufficient,
            s.adapter_watts,
            s.adapter_volts,
            s.watts().unwrap_or(0.) > 0.,
            connected_draining(s),
        );
        if self.panel.is_some() && self.panel_mode != Some(mode) {
            let pressed: usize = msg_send![class!(NSEvent), pressedMouseButtons];
            if pressed != 0 {
                self.pending_rebuild = true;
                return;
            }
            self.rebuild();
            return;
        }
        let s = &self.store.snapshot;
        let mut parts = Vec::new();
        if s.stale || !s.has_reading || connected_draining(s) {
            parts.push("⚠".into());
        }
        if !self.store.settings.flag("bar_compact") {
            if self.store.settings.flag("bar_percentage") && s.has_reading {
                parts.push(format!("{}%", s.percent));
            }
            if self.store.settings.flag("bar_time") {
                if let Some(h) = s.hours.filter(|h| *h > 0. && *h < 48.) {
                    parts.push(format!("≈{}", model::duration(h)));
                }
            }
            if self.store.settings.flag("bar_wattage") {
                if let Some(w) = s.average.filter(|w| *w > 0.) {
                    parts.push(format!("{:.0}W", w));
                }
            }
        }
        let text = parts.join("  ");
        let button: id = msg_send![self.status, button];
        if self.bar_text != text {
            let _: () = msg_send![button,setTitle:string(&text)];
            self.bar_text = text;
        }
        let key = (
            s.percent,
            s.state,
            s.has_reading,
            connected_draining(s),
            s.watts().is_some_and(|w| w > 0.),
        );
        if self.bar_image_key != key {
            self.bar_image_key = key;
            if s.has_reading {
                let image = NSImage::alloc(nil).initWithSize_(NSSize::new(28., 17.));
                image.lockFocus();
                let (r, g, b) = tint(s);
                rounded(rect(1., 2., 23., 12.), 3., color(r, g, b, 0.16));
                let outline: id = msg_send![class!(NSBezierPath),bezierPathWithRoundedRect:rect(1.,2.,23.,12.) xRadius:3.0_f64 yRadius:3.0_f64];
                set_color(color(r, g, b, 0.9));
                let _: () = msg_send![outline,setLineWidth:1.0_f64];
                let _: () = msg_send![outline, stroke];
                rounded(
                    rect(3., 4., 19. * s.percent as f64 / 100., 8.),
                    1.5,
                    color(r, g, b, 1.),
                );
                rounded(rect(25., 6., 2., 4.), 1., color(r, g, b, 0.7));
                if connected_draining(s) {
                    let p: id = msg_send![class!(NSBezierPath), bezierPath];
                    let _: () = msg_send![p,moveToPoint:NSPoint::new(13.,13.)];
                    let _: () = msg_send![p,lineToPoint:NSPoint::new(13.,5.)];
                    let _: () = msg_send![p,moveToPoint:NSPoint::new(9.,8.)];
                    let _: () = msg_send![p,lineToPoint:NSPoint::new(13.,4.)];
                    let _: () = msg_send![p,lineToPoint:NSPoint::new(17.,8.)];
                    let _: () = msg_send![p,setLineWidth:1.7_f64];
                    set_color(color(1., 1., 1., 1.));
                    let _: () = msg_send![p, stroke];
                } else if s.state == Power::Plugged {
                    let plug: id = msg_send![class!(NSImage), imageWithSystemSymbolName:string("powerplug.fill") accessibilityDescription:nil];
                    let _: () = msg_send![plug, drawInRect:rect(7., 2., 12., 12.)];
                } else if s.state == Power::Charged {
                    let p: id = msg_send![class!(NSBezierPath), bezierPath];
                    let _: () = msg_send![p,moveToPoint:NSPoint::new(8.,8.)];
                    let _: () = msg_send![p,lineToPoint:NSPoint::new(11.,5.)];
                    let _: () = msg_send![p,lineToPoint:NSPoint::new(18.,12.)];
                    let _: () = msg_send![p,setLineWidth:1.7_f64];
                    let c: id = msg_send![class!(NSColor), labelColor];
                    set_color(c);
                    let _: () = msg_send![p, stroke];
                } else if s.state == Power::Charging {
                    let p: id = msg_send![class!(NSBezierPath), bezierPath];
                    let _: () = msg_send![p,moveToPoint:NSPoint::new(14.,14.)];
                    let _: () = msg_send![p,lineToPoint:NSPoint::new(9.,7.)];
                    let _: () = msg_send![p,lineToPoint:NSPoint::new(13.,7.)];
                    let _: () = msg_send![p,lineToPoint:NSPoint::new(10.,1.)];
                    let _: () = msg_send![p,lineToPoint:NSPoint::new(18.,10.)];
                    let _: () = msg_send![p,lineToPoint:NSPoint::new(14.,10.)];
                    let _: () = msg_send![p, closePath];
                    set_color(color(1., 1., 1., 1.));
                    let _: () = msg_send![p, fill];
                }
                image.unlockFocus();
                let _: () = msg_send![button,setImage:image];
                let _: () = msg_send![image, release];
            } else {
                let _: () = msg_send![button,setImage:nil];
            }
        }
        // NSStatusBarButton resolves its foreground from the menu-bar backdrop,
        // which can differ from the application's Light/Dark appearance. A
        // contentTintColor override also forces the title, making it unreadable
        // on a dark bar when the application resolves labelColor as black.
        // Keep the native foreground; battery-state colours remain in the image.
        let _: () = msg_send![button,setContentTintColor:nil];
        let tip = format!("BatteryBar · {} · {}%", visible_power_state(s), s.percent);
        let _: () = msg_send![button,setToolTip:string(&tip)];
        if let Some(panel) = self.panel.as_ref() {
            let state = visible_power_state(s);
            let values = [
                ("state", state.to_string()),
                (
                    "updated",
                    self.store
                        .last_update
                        .map(|t| {
                            let age = (model::now() - t) as i64;
                            if age < 8 {
                                "aggiornato ora".into()
                            } else {
                                format!("aggiornato {age}s fa")
                            }
                        })
                        .unwrap_or_default(),
                ),
                ("percent", format!("{}%", s.percent)),
                (
                    "estimate",
                    s.hours
                        .filter(|h| *h > 0. && *h < 48.)
                        .map(|h| format!("≈ {}", model::duration(h)))
                        .unwrap_or_else(|| {
                            if connected_draining(s) {
                                return "Calcolo in corso".into();
                            }
                            match s.state {
                                Power::Charged => "Pronto",
                                Power::Plugged => "Carica in pausa",
                                _ => "Calcolo in corso",
                            }
                            .into()
                        }),
                ),
                (
                    "eta",
                    s.eta
                        .map(|e| format!("verso le {}", clock(e)))
                        .unwrap_or_default(),
                ),
                ("quality", s.quality.clone()),
                (
                    "watts",
                    s.watts()
                        .map(|w| format!("{:.1} W", w.abs()))
                        .unwrap_or("--".into()),
                ),
                (
                    "health",
                    s.health().map(|h| format!("{h}%")).unwrap_or("--".into()),
                ),
                (
                    "cycles",
                    s.cycles.map(|v| v.to_string()).unwrap_or("--".into()),
                ),
                (
                    "temperature",
                    s.temperature
                        .map(|v| format!("{v:.1}°C"))
                        .unwrap_or("--".into()),
                ),
                (
                    "warning",
                    if s.stale || !s.has_reading {
                        "Lettura non disponibile. L'ultimo dato valido resta visibile.".into()
                    } else if s.insufficient {
                        "La batteria si scarica nonostante il cavo: alimentazione forse insufficiente.".into()
                    } else if connected_draining(s) {
                        "La batteria sta perdendo energia, anche se il cavo è collegato.".into()
                    } else {
                        String::new()
                    },
                ),
            ];
            for (key, value) in values {
                if let Some(l) = panel.labels.get(key) {
                    message_text(*l, &value);
                }
            }
            let _: () = msg_send![panel.gauge,setNeedsDisplay:YES];
            let _: () = msg_send![panel.chart,setNeedsDisplay:YES];
        }
    }
    unsafe fn tick(&mut self) {
        let _pool = Pool::new();
        let t = model::now();
        if self.options.show_panel && t - self.start >= 1.0 {
            self.options.show_panel = false;
            self.toggle();
        }
        if self.options.quit_after.is_some() && t - self.last_memory >= self.options.memory_every {
            let mut sample = platform::memory();
            sample["since_start"] = serde_json::json!(t - self.start);
            sample["ui"] = activity_table::diagnostics();
            sample["panel_open"] = serde_json::json!(self.panel.is_some());
            sample["expanded"] = serde_json::json!(self.show_all);
            if let Some(panel) = self.panel.as_ref() {
                let children: id = msg_send![panel.root, subviews];
                let n: usize = msg_send![children, count];
                sample["root_subviews"] = serde_json::json!(n);
                let root_layer: id = msg_send![panel.root, layer];
                let contents: id = msg_send![root_layer, contents];
                sample["root_contents_empty"] = serde_json::json!(contents == nil);
                if panel.table != nil {
                    let extent_layer: id = msg_send![panel.table, layer];
                    let contents: id = msg_send![extent_layer, contents];
                    sample["list_extent_contents_empty"] = serde_json::json!(contents == nil);
                }
            }
            if self.memory_samples.len() == 64 {
                self.memory_samples.remove(0);
            }
            self.memory_samples.push(sample);
            self.last_memory = t;
        }
        if self
            .options
            .close_after
            .is_some_and(|seconds| t - self.start >= seconds)
        {
            self.options.close_after = None;
            self.close("scheduled");
        }
        if self.cycle_step < self.options.panel_cycles * 2
            && t - self.start >= 1. + self.cycle_step as f64 * 2.
        {
            let expanding = self.options.expanded_cycles && self.cycle_step.is_multiple_of(2);
            if expanding {
                self.show_settings = true;
                self.show_all = true;
            }
            self.toggle();
            if expanding {
                if let Some(panel) = self.panel.as_ref() {
                    let scroll: id = msg_send![panel.controller, view];
                    let clip: id = msg_send![scroll, contentView];
                    let _: () = msg_send![clip,scrollToPoint:NSPoint::new(0.,400.)];
                    let _: () = msg_send![scroll,reflectScrolledClipView:clip];
                }
            }
            self.cycle_step += 1;
        }
        if self.options.regression_scroll
            && self.regression.is_none()
            && t - self.start >= 5.
            && self.panel.is_some()
        {
            self.store
                .settings
                .set("activity_history_hours", serde_json::json!(6));
            self.store
                .settings
                .set("activity_group_apps", serde_json::json!(false));
            self.show_settings = true;
            self.show_all = true;
            self.report = self.store.report();
            self.rebuild();
            let p = self.panel.as_ref().unwrap();
            let scroll: id = msg_send![p.controller, view];
            let clip: id = msg_send![scroll, contentView];
            let _: () = msg_send![clip,scrollToPoint:NSPoint::new(0.,400.)];
            let _: () = msg_send![scroll,reflectScrolledClipView:clip];
            let bounds: NSRect = msg_send![clip, bounds];
            let window: id = msg_send![scroll, window];
            let control = child_tag(p.root, 10);
            let _: bool = msg_send![window,makeFirstResponder:control];
            self.regression = Some(Regression {
                offset: bounds.origin.y,
                period: self.store.settings.int("activity_history_hours"),
                group: self.store.settings.flag("activity_group_apps"),
                settings: self.show_settings,
                expanded: self.show_all,
                focus: responder_tag(window),
                collected: self.last_collect,
                checked: false,
            });
        }
        if self.options.regression_scroll
            && self.regression.is_some()
            && self.sweep_step < 4
            && t - self.start >= 8. + self.sweep_step as f64 * 5.
        {
            if let Some(panel) = self.panel.as_ref() {
                let table = panel.table;
                if table != nil {
                    let frame: NSRect = msg_send![table, frame];
                    let row = match self.sweep_step {
                        0 => 0.,
                        1 => panel.activity_rows as f64 / 2.,
                        2 => panel.activity_rows.saturating_sub(10) as f64,
                        _ => 0.,
                    };
                    let y = if self.sweep_step == 3 {
                        self.regression.as_ref().unwrap().offset
                    } else {
                        frame.origin.y + row * 23.
                    };
                    let scroll: id = msg_send![panel.controller, view];
                    let clip: id = msg_send![scroll, contentView];
                    let _: () = msg_send![clip,scrollToPoint:NSPoint::new(0.,y)];
                    let _: () = msg_send![scroll,reflectScrolledClipView:clip];
                    self.trace("scroll_sweep");
                    self.sweep_step += 1;
                }
            }
        }
        if t - self.last_settle >= 0.95 || (t < self.settle_until && t - self.last_settle >= 0.24) {
            self.store.refresh();
            self.last_settle = t;
            self.update();
        }
        if t - self.last_collect >= 30. {
            self.store.collect();
            self.last_collect = t;
            if self.panel.is_some() {
                self.report = self.store.report();
                let pressed: usize = msg_send![class!(NSEvent), pressedMouseButtons];
                if pressed == 0 {
                    self.refresh_report();
                } else {
                    self.pending_report = true;
                }
            }
        }
        if self.pending_rebuild && self.panel.is_some() {
            let pressed: usize = msg_send![class!(NSEvent), pressedMouseButtons];
            if pressed == 0 {
                self.pending_rebuild = false;
                self.rebuild();
            }
        }
        if self.pending_report && self.panel.is_some() {
            let pressed: usize = msg_send![class!(NSEvent), pressedMouseButtons];
            if pressed == 0 {
                self.pending_report = false;
                self.refresh_report();
            }
        }
        if let Some(r) = self.regression.as_mut() {
            if !r.checked && t - self.start >= 40. && self.last_collect > r.collected {
                let mut offset = -1.;
                let mut focus = 0;
                let shown: bool = msg_send![self.popover, isShown];
                if let Some(p) = self.panel.as_ref() {
                    let scroll: id = msg_send![p.controller, view];
                    let clip: id = msg_send![scroll, contentView];
                    let bounds: NSRect = msg_send![clip, bounds];
                    offset = bounds.origin.y;
                    let window: id = msg_send![scroll, window];
                    focus = responder_tag(window);
                }
                let pass = (offset - r.offset).abs() < 0.5
                    && self.store.settings.int("activity_history_hours") == r.period
                    && self.store.settings.flag("activity_group_apps") == r.group
                    && self.show_settings == r.settings
                    && self.show_all == r.expanded
                    && focus == r.focus
                    && shown;
                self.ui_checks.push(serde_json::json!({"check":"report_refresh_preserves_reading_state","pass":pass,"offset_before":r.offset,"offset_after":offset,"focus_before":r.focus,"focus_after":focus,"period_hours":r.period,"group_apps":r.group,"settings":self.show_settings,"expanded":self.show_all,"popover_shown":shown}));
                if let Some(panel) = self.panel.as_ref() {
                    if panel.table != nil {
                        self.ui_checks.push(activity_table::validate_access(
                            panel.table,
                            &self.report.rows[..panel.activity_rows],
                        ));
                    }
                }
                r.checked = true;
            }
        }
        if self.panel.is_some() {
            let shown: bool = msg_send![self.popover, isShown];
            if shown == NO {
                self.trace("cleanup_not_shown");
                self.close("popover_closed");
            }
        }
    }
    unsafe fn toggle(&mut self) {
        let shown: bool = msg_send![self.popover, isShown];
        if shown == YES {
            self.close("toggle");
            return;
        }
        self.store.refresh();
        self.report = self.store.report();
        self.rebuild();
        let b: id = msg_send![self.status, button];
        let bounds: NSRect = msg_send![b, bounds];
        let _: () =
            msg_send![self.popover,showRelativeToRect:bounds ofView:b preferredEdge:1_usize];
        let _: () = msg_send![NSApp(), activate];
        let controller: id = msg_send![self.popover, contentViewController];
        let view: id = msg_send![controller, view];
        let window: id = msg_send![view, window];
        let _: () = msg_send![window, makeKeyWindow];
        if let Some(panel) = self.panel.as_ref() {
            activity_table::sync(panel.table);
        }
        if !self.options.hold_panel {
            platform::dismiss_watch(true);
        }
        self.trace("shown");
        self.update();
    }
    unsafe fn close(&mut self, reason: &str) {
        self.trace(&format!("close_{reason}"));
        platform::dismiss_watch(false);
        let _: () = msg_send![self.popover,performClose:nil];
        let _: () = msg_send![self.popover,setContentViewController:nil];
        self.panel = None;
        self.report = Report::default();
        self.show_settings = false;
        self.show_all = false;
    }
    unsafe fn update_consumers(&self) {
        let Some(panel) = self.panel.as_ref() else {
            return;
        };
        let empty = self.store.consumers.rows.is_empty();
        if let Some(label) = panel.labels.get("empty_consumers") {
            message_text(
                *label,
                if self.store.activity_updated.is_none() {
                    "Primo campione in raccolta (30 secondi)"
                } else {
                    "Nessuna attività rilevata"
                },
            );
            let _: () = msg_send![*label,setHidden:!empty];
        }
        let seconds = self.store.consumers.seconds.max(0.001);
        for (i, views) in panel.consumers.iter().enumerate() {
            let row = self.store.consumers.rows.get(i);
            for view in [views.name, views.cpu, views.watts, views.progress] {
                let _: () = msg_send![view,setHidden:row.is_none()];
            }
            if let Some(row) = row {
                let cpu = row.cpu / seconds * 100.;
                message_text(views.name, row.name());
                let _: () = msg_send![views.name,setToolTip:string(&row.path)];
                message_text(views.cpu, &format!("{cpu:.0}%"));
                message_text(
                    views.watts,
                    &row.joules
                        .map(|j| format!("{:.2} W", j / seconds))
                        .unwrap_or("n/d".into()),
                );
                let _: () = msg_send![views.progress,setDoubleValue:cpu.min(100.)];
            }
        }
    }
    unsafe fn refresh_report(&mut self) {
        let Some(panel) = self.panel.as_ref() else {
            return;
        };
        let chart_hidden: bool = msg_send![panel.chart, isHidden];
        if (panel.table != nil) != (self.report.seconds > 0.)
            || panel.labels.contains_key("show_all") != (self.report.rows.len() > 8)
            || panel.labels.contains_key("report_error") != self.store.activity_error.is_some()
            || chart_hidden != (self.store.history.len() <= 2)
        {
            self.rebuild();
            return;
        }
        let count = self
            .report
            .rows
            .len()
            .min(if self.show_all { usize::MAX } else { 8 });
        let panel = self.panel.as_mut().unwrap();
        if panel.table != nil {
            let scroll: id = msg_send![panel.controller, view];
            let clip: id = msg_send![scroll, contentView];
            let old_bounds: NSRect = msg_send![clip, bounds];
            let frame: NSRect = msg_send![panel.table, frame];
            let bottom = frame.origin.y + frame.size.height;
            let delta = (count as f64 - panel.activity_rows as f64) * 23.;
            if delta.abs() > 0.01 {
                let children: id = msg_send![panel.root, subviews];
                let n: usize = msg_send![children, count];
                for i in 0..n {
                    let child: id = msg_send![children,objectAtIndex:i];
                    if child == panel.table {
                        continue;
                    }
                    let r: NSRect = msg_send![child, frame];
                    if r.origin.y >= bottom - 0.01 {
                        let _: () = msg_send![child,setFrameOrigin:NSPoint::new(r.origin.x,r.origin.y+delta)];
                    }
                }
                let r: NSRect = msg_send![panel.root, frame];
                let _: () = msg_send![panel.root,setFrameSize:NSSize::new(r.size.width,r.size.height+delta)];
                let _: () = msg_send![panel.table,setFrameSize:NSSize::new(frame.size.width,count as f64*23.)];
                let max_y = (r.size.height + delta - old_bounds.size.height).max(0.);
                let _: () =
                    msg_send![clip,scrollToPoint:NSPoint::new(0.,old_bounds.origin.y.min(max_y))];
                let _: () = msg_send![scroll,reflectScrolledClipView:clip];
            }
            activity_table::update(panel.table, &self.report.rows[..count]);
            panel.activity_rows = count;
            if let Some(label) = panel.labels.get("coverage") {
                message_text(
                    *label,
                    &format!(
                        "Copertura: {} registrati. Energia CPU: {}",
                        model::activity_duration(self.report.seconds),
                        model::energy(self.report.rows.iter().filter_map(|r| r.joules).sum())
                    ),
                );
            }
            if let Some(label) = panel.labels.get("usb") {
                message_text(
                    *label,
                    &format!(
                        "USB presenti negli intervalli registrati: {}",
                        if self.report.devices.is_empty() {
                            "nessuna rilevata".into()
                        } else {
                            self.report.devices.join(", ")
                        }
                    ),
                );
            }
            if let Some(button) = panel.labels.get("show_all") {
                let title = if self.show_all {
                    "Mostra meno".into()
                } else {
                    format!("Mostra tutti ({})", self.report.rows.len())
                };
                let _: () = msg_send![*button,setTitle:string(&title)];
                let _: () = msg_send![*button,setNeedsDisplay:YES];
            }
        }
        if let (Some(label), Some(error)) = (
            panel.labels.get("report_error"),
            self.store.activity_error.as_ref(),
        ) {
            message_text(*label, error);
        }
        self.update_consumers();
        self.trace("report_updated_in_place");
    }
    unsafe fn action(&mut self, sender: id) {
        let tag: i64 = msg_send![sender, tag];
        match tag {
            1 => {
                self.store.refresh();
                self.update();
                return;
            }
            2 => {
                let _: () = msg_send![NSApp(),terminate:nil];
                return;
            }
            3 => self.show_settings = !self.show_settings,
            4 => self.show_all = !self.show_all,
            5 | 6 => {
                let url = if tag == 5 {
                    "x-apple.systempreferences:com.apple.preference.battery"
                } else {
                    "https://ko-fi.com/shaungioelegori"
                };
                let u: id = msg_send![class!(NSURL),URLWithString:string(url)];
                let w: id = msg_send![class!(NSWorkspace), sharedWorkspace];
                let _: bool = msg_send![w,openURL:u];
                return;
            }
            10 => {
                let i: isize = msg_send![sender, selectedSegment];
                if let Some(h) = [1, 6, 12, 24, 48].get(i as usize) {
                    self.store
                        .settings
                        .set("activity_history_hours", serde_json::json!(h));
                }
            }
            11 => {
                let i: isize = msg_send![sender, selectedSegment];
                self.store
                    .settings
                    .set("activity_group_apps", serde_json::json!(i == 0));
            }
            20..=27 => {
                let keys = [
                    "bar_compact",
                    "bar_percentage",
                    "bar_time",
                    "bar_wattage",
                    "notify_full",
                    "notify_low",
                    "notify_limit",
                    "track_history",
                ];
                self.store.toggle(keys[(tag - 20) as usize]);
            }
            28 => {
                if self.options.isolated {
                    self.store.activity_error = Some(
                        "Avvio al login disponibile dopo l'installazione del bundle definitivo."
                            .into(),
                    );
                } else if let Err(e) = platform::set_login(!platform::login()) {
                    self.store.activity_error = Some(e);
                }
            }
            30 => {
                let i: isize = msg_send![sender, selectedSegment];
                if let Some(n) = [10, 15, 20, 30].get(i as usize) {
                    self.store
                        .settings
                        .set("notify_low_pct", serde_json::json!(n));
                }
            }
            31 => {
                let i: isize = msg_send![sender, selectedSegment];
                if let Some(n) = [70, 80, 85, 90].get(i as usize) {
                    self.store
                        .settings
                        .set("notify_limit_pct", serde_json::json!(n));
                }
            }
            _ => {}
        }
        self.report = self.store.report();
        self.rebuild();
        self.update();
    }
    unsafe fn rebuild(&mut self) {
        self.pending_rebuild = false;
        self.pending_report = false;
        self.trace("rebuild_begin");
        let focus = self
            .panel
            .as_ref()
            .map(|p| {
                let window: id = msg_send![p.root, window];
                responder_tag(window)
            })
            .unwrap_or(0);
        let old_scroll = self
            .panel
            .as_ref()
            .map(|p| {
                let v: id = msg_send![p.controller, view];
                let clip: id = msg_send![v, contentView];
                let bounds: NSRect = msg_send![clip, bounds];
                bounds.origin.y
            })
            .unwrap_or(0.);
        let s = &self.store.snapshot;
        let row_count = self
            .report
            .rows
            .len()
            .min(if self.show_all { usize::MAX } else { 8 });
        let report_h = if self.report.seconds > 0. {
            row_count as f64 * 23. + 88.
        } else {
            38.
        };
        let total = 666. + report_h + if self.show_settings { 462. } else { 0. };
        let root: id = msg_send![class!(BatteryBarRustFlipped), alloc];
        let root: id = msg_send![root,initWithFrame:rect(0.,0.,380.,total)];
        let _: () = msg_send![root,setWantsLayer:YES];
        let mut labels = HashMap::new();
        card(root, rect(12., 103., 356., 110.), 14.);
        for i in 0..4 {
            let x = 16. + (i % 2) as f64 * 180.;
            let y = 234. + (i / 2) as f64 * 49.;
            card(root, rect(x, y, 168., 45.), 10.);
        }
        label(
            root,
            "Batteria",
            rect(16., 14., 170., 22.),
            15.,
            true,
            false,
        );
        labels.insert(
            "state",
            label(
                root,
                visible_power_state(s),
                rect(16., 38., 306., 18.),
                10.,
                true,
                true,
            ),
        );
        labels.insert(
            "updated",
            label(
                root,
                "aggiornato ora",
                rect(16., 57., 240., 14.),
                10.,
                false,
                true,
            ),
        );
        button(root, "↻", rect(326., 24., 38., 30.), self.target, 1);
        labels.insert("warning", multiline(root, "", 16., 77., 348., 32., 10.));
        let gauge: id = msg_send![class!(BatteryBarRustGauge), alloc];
        let gauge: id = msg_send![gauge,initWithFrame:rect(18.,109.,88.,88.)];
        let _: () = msg_send![root,addSubview:gauge];
        let _: () = msg_send![gauge, release];
        let p = label(
            root,
            &format!("{}%", s.percent),
            rect(32., 139., 65., 30.),
            21.,
            true,
            false,
        );
        let _: () = msg_send![p,setAlignment:1_usize];
        labels.insert("percent", p);
        let symbol = power_symbol(s);
        let image: id = msg_send![class!(NSImage),imageWithSystemSymbolName:string(symbol) accessibilityDescription:nil];
        let icon: id = msg_send![class!(NSImageView), alloc];
        let icon: id = msg_send![icon,initWithFrame:rect(53.,173.,16.,13.)];
        let _: () = msg_send![icon,setImage:image];
        let _: () = msg_send![root,addSubview:icon];
        let _: () = msg_send![icon, release];
        label(
            root,
            estimate_heading(s),
            rect(122., 111., 242., 16.),
            9.,
            true,
            true,
        );
        // Gauge occupies the left column; the estimate follows the original panel hierarchy.

        labels.insert(
            "estimate",
            label(root, "", rect(122., 132., 242., 28.), 21., true, false),
        );
        labels.insert(
            "eta",
            label(root, "", rect(122., 162., 242., 17.), 11., false, true),
        );
        labels.insert(
            "quality",
            label(root, "", rect(122., 181., 242., 16.), 9., false, true),
        );
        if s.state == Power::Charging && !connected_draining(s) {
            label(
                root,
                "La carica può rallentare verso il 100%.",
                rect(122., 198., 242., 15.),
                9.,
                false,
                true,
            );
        }
        if s.state == Power::Charging && !connected_draining(s) {
            let spinner: id = msg_send![class!(NSProgressIndicator), alloc];
            let spinner: id = msg_send![spinner,initWithFrame:rect(88.,111.,12.,12.)];
            let _: () = msg_send![spinner,setStyle:1_usize];
            let _: () = msg_send![spinner,setControlSize:1_usize];
            let _: () = msg_send![spinner,startAnimation:nil];
            let _: () = msg_send![root,addSubview:spinner];
            let _: () = msg_send![spinner, release];
        }
        if let Some(delta) = self.store.hour_delta().filter(|d| *d != 0) {
            label(
                root,
                &format!(
                    "{} {}% nell'ultima ora",
                    if delta > 0 { "↗" } else { "↘" },
                    delta.abs()
                ),
                rect(16., 215., 348., 18.),
                10.,
                true,
                true,
            );
        }
        for (i, (key, title)) in [
            (
                "watts",
                if s.watts().unwrap_or(0.) > 0. {
                    "verso batteria"
                } else {
                    "dalla batteria"
                },
            ),
            ("health", "salute"),
            ("cycles", "cicli"),
            ("temperature", "temperatura"),
        ]
        .iter()
        .enumerate()
        {
            let x = 16. + (i % 2) as f64 * 180.;
            let y = 238. + (i / 2) as f64 * 49.;
            let symbol = [
                "bolt.fill",
                "heart.fill",
                "arrow.triangle.2.circlepath",
                "thermometer.medium",
            ][i];
            let icon_tint = if i == 0 {
                (0.52, 0.60, 0.98)
            } else {
                (0.18, 0.72, 0.61)
            };
            metric_icon(root, symbol, rect(x + 9., y + 12., 15., 15.), icon_tint);
            label(root, title, rect(x + 30., y, 130., 14.), 9., false, true);
            labels.insert(
                *key,
                label(
                    root,
                    "--",
                    rect(x + 30., y + 15., 130., 24.),
                    17.,
                    true,
                    false,
                ),
            );
        }
        let mut adapter = Vec::new();
        if let Some(w) = s.adapter_watts {
            adapter.push(format!("{w}W"));
        }
        if let Some(v) = s.adapter_volts {
            adapter.push(format!("{v}V"));
        }
        if s.state != Power::Discharging {
            label(
                root,
                &format!(
                    "Alimentatore  {}",
                    if adapter.is_empty() {
                        "collegato".into()
                    } else {
                        adapter.join(" · ")
                    }
                ),
                rect(16., 340., 348., 23.),
                11.,
                true,
                true,
            );
        }
        section(root, "ATTIVITÀ CPU · ULTIMI 30 SECONDI", 371.);
        labels.insert(
            "empty_consumers",
            label(root, "", rect(16., 390., 348., 18.), 10., false, true),
        );
        let consumers = std::array::from_fn(|i| {
            let y = 390. + i as f64 * 23.;
            let name = label(root, "", rect(16., y, 148., 20.), 11., false, false);
            let cpu = label(root, "", rect(252., y, 46., 20.), 10., true, true);
            let watts = label(root, "", rect(304., y, 60., 20.), 9., false, true);
            let progress: id = msg_send![class!(NSProgressIndicator), alloc];
            let progress: id = msg_send![progress,initWithFrame:rect(164.,y+7.,80.,6.)];
            let _: () = msg_send![progress,setIndeterminate:NO];
            let _: () = msg_send![root,addSubview:progress];
            let _: () = msg_send![progress, release];
            ConsumerViews {
                name,
                cpu,
                watts,
                progress,
            }
        });
        multiline(
            root,
            "100% = un core. Watt della sola CPU attribuiti da macOS, non consumo totale dell'app.",
            16.,
            528.,
            348.,
            30.,
            9.,
        );
        let mut y = 566.;
        section(root, "STORICO ATTIVITÀ ED ENERGIA CPU", y);
        y += 21.;
        let hours = self.store.settings.int("activity_history_hours");
        segments(
            root,
            &[1, 6, 12, 24, 48]
                .iter()
                .map(|h| format!("{h}h"))
                .collect::<Vec<_>>(),
            [1, 6, 12, 24, 48]
                .iter()
                .position(|h| *h == hours)
                .unwrap_or(3),
            y,
            self.target,
            10,
        );
        y += 31.;
        segments(
            root,
            &["App".into(), "Processi".into()],
            if self.store.settings.flag("activity_group_apps") {
                0
            } else {
                1
            },
            y,
            self.target,
            11,
        );
        y += 34.;
        let mut activity_view = nil;
        if self.report.seconds > 0. {
            labels.insert(
                "coverage",
                multiline(
                    root,
                    &format!(
                        "Copertura: {} registrati. Energia CPU: {}",
                        model::activity_duration(self.report.seconds),
                        model::energy(self.report.rows.iter().filter_map(|r| r.joules).sum())
                    ),
                    16.,
                    y,
                    348.,
                    28.,
                    10.,
                ),
            );
            y += 31.;
            activity_view = activity_table::create(
                root,
                &self.report.rows[..row_count],
                rect(16., y, 348., row_count as f64 * 23.),
            );
            y += row_count as f64 * 23.;
            if self.report.rows.len() > 8 {
                labels.insert(
                    "show_all",
                    button(
                        root,
                        &if self.show_all {
                            "Mostra meno".into()
                        } else {
                            format!("Mostra tutti ({})", self.report.rows.len())
                        },
                        rect(16., y, 180., 25.),
                        self.target,
                        4,
                    ),
                );
                y += 28.;
            }
            labels.insert(
                "usb",
                multiline(
                    root,
                    &format!(
                        "USB presenti negli intervalli registrati: {}",
                        if self.report.devices.is_empty() {
                            "nessuna rilevata".into()
                        } else {
                            self.report.devices.join(", ")
                        }
                    ),
                    16.,
                    y,
                    348.,
                    35.,
                    9.,
                ),
            );
            y += 39.;
        } else {
            multiline(
                root,
                if self.store.settings.flag("track_history") {
                    "Lo storico si popola da questo aggiornamento, senza ricostruire consumi passati."
                } else {
                    "Registrazione storico disattivata."
                },
                16.,
                y,
                348.,
                32.,
                10.,
            );
            y += 36.;
        }
        multiline(root,"Energia CPU parziale: esclusi GPU, schermo e processi non accessibili o troppo brevi. I Wh di USB, Bluetooth e altre periferiche non sono disponibili; non vengono stimati arbitrariamente.",16.,y,348.,48.,9.);
        y += 52.;
        if let Some(error) = &self.store.activity_error {
            labels.insert(
                "report_error",
                multiline(root, error, 16., y, 348., 36., 10.),
            );
            y += 40.;
        }
        let chart: id = msg_send![class!(BatteryBarRustChart), alloc];
        let chart: id = msg_send![chart,initWithFrame:rect(16.,y+20.,348.,95.)];
        let _: () = msg_send![root,addSubview:chart];
        if self.store.history.len() > 2 {
            section(root, &format!("BATTERIA · ULTIME {hours} ORE"), y);
            label(root, "100", rect(340., y + 20., 32., 15.), 8., false, true);
            label(root, "50", rect(340., y + 57., 28., 15.), 8., false, true);
            label(root, "0", rect(340., y + 94., 28., 15.), 8., false, true);
            for i in 0..=4 {
                let time = model::now() - (hours as f64 * 3600.) * (1. - i as f64 / 4.);
                label(
                    root,
                    &clock(time),
                    rect(16. + i as f64 * 78., y + 112., 42., 15.),
                    8.,
                    false,
                    true,
                );
            }
            y += 135.;
        } else {
            let _: () = msg_send![chart,setHidden:YES];
        }
        let _: () = msg_send![chart, release];
        button(
            root,
            if self.show_settings {
                "Impostazioni ▴"
            } else {
                "Impostazioni ▾"
            },
            rect(16., y, 155., 28.),
            self.target,
            3,
        );
        button(root, "⚙", rect(178., y, 38., 28.), self.target, 5);
        button(root, "☕", rect(274., y, 38., 28.), self.target, 6);
        button(root, "⏻", rect(326., y, 38., 28.), self.target, 2);
        y += 42.;
        if self.show_settings {
            section(root, "NELLA BARRA", y);
            y += 20.;
            for (i, (title, key)) in [
                ("Solo anello", "bar_compact"),
                ("Percentuale", "bar_percentage"),
                ("Tempo", "bar_time"),
                ("Watt", "bar_wattage"),
            ]
            .iter()
            .enumerate()
            {
                let b = button(
                    root,
                    &format!(
                        "{}{}",
                        if self.store.settings.flag(key) {
                            "✓ "
                        } else {
                            ""
                        },
                        title
                    ),
                    rect(16. + i as f64 * 89., y, 86., 27.),
                    self.target,
                    20 + i as i64,
                );
                let _: () = msg_send![b,setToolTip:string(title)];
            }
            y += 40.;
            for (title, key, tag) in [
                ("Avvisa a carica completa", "notify_full", 24),
                ("Avvisa a batteria bassa", "notify_low", 25),
                ("Avvisa al limite di carica", "notify_limit", 26),
                ("Registra lo storico", "track_history", 27),
            ] {
                toggle(
                    root,
                    title,
                    y,
                    self.store.settings.flag(key),
                    self.target,
                    tag,
                );
                y += 32.;
            }
            toggle(
                root,
                "Avvia al login",
                y,
                platform::login(),
                self.target,
                28,
            );
            y += 42.;
            section(root, "SOGLIA BATTERIA BASSA", y);
            y += 21.;
            segments(
                root,
                &[10, 15, 20, 30]
                    .iter()
                    .map(|n| format!("{n}%"))
                    .collect::<Vec<_>>(),
                [10, 15, 20, 30]
                    .iter()
                    .position(|n| *n == self.store.settings.int("notify_low_pct"))
                    .unwrap_or(2),
                y,
                self.target,
                30,
            );
            y += 40.;
            section(root, "LIMITE DI CARICA", y);
            y += 21.;
            segments(
                root,
                &[70, 80, 85, 90]
                    .iter()
                    .map(|n| format!("{n}%"))
                    .collect::<Vec<_>>(),
                [70, 80, 85, 90]
                    .iter()
                    .position(|n| *n == self.store.settings.int("notify_limit_pct"))
                    .unwrap_or(1),
                y,
                self.target,
                31,
            );
            y += 40.;
        }
        let _: () = msg_send![root,setFrameSize:NSSize::new(380.,y+8.)];
        let controller: id = if let Some(panel) = self.panel.as_ref() {
            let _: () = msg_send![panel.controller, retain];
            panel.controller
        } else {
            msg_send![class!(NSViewController), new]
        };
        let scroll: id = if self.panel.is_some() {
            let scroll: id = msg_send![controller, view];
            let _: () = msg_send![scroll, retain];
            scroll
        } else {
            let scroll: id = msg_send![class!(NSScrollView), alloc];
            msg_send![scroll,initWithFrame:rect(0.,0.,380.,(y+8.).min(620.))]
        };
        let _: () = msg_send![scroll,setHasVerticalScroller:YES];
        let _: () = msg_send![scroll,setDrawsBackground:YES];
        let background: id = msg_send![class!(NSColor), windowBackgroundColor];
        let _: () = msg_send![scroll,setBackgroundColor:background];
        let _: () = msg_send![scroll,setAutohidesScrollers:YES];
        let _: () = msg_send![scroll,setDocumentView:root];
        if self.panel.is_none() {
            let _: () = msg_send![controller,setView:scroll];
            let _: () = msg_send![self.popover,setContentViewController:controller];
        }
        let _: () = msg_send![self.popover,setContentSize:NSSize::new(380.,(y+8.).min(620.))];
        let clip: id = msg_send![scroll, contentView];
        let _: () =
            msg_send![clip,scrollToPoint:NSPoint::new(0.,old_scroll.min((y+8.-620.).max(0.)))];
        let _: () = msg_send![scroll,reflectScrolledClipView:clip];
        activity_table::attach(activity_view, scroll);
        if focus != 0 {
            let handle = child_tag(root, focus);
            let control = if focus == 40 && handle != nil {
                activity_table::focus_target(handle)
            } else {
                handle
            };
            if control != nil {
                let window: id = msg_send![scroll, window];
                let _: bool = msg_send![window,makeFirstResponder:control];
            }
        }
        self.panel = Some(Panel {
            controller,
            root,
            labels,
            gauge,
            chart,
            table: activity_view,
            consumers,
            activity_rows: row_count,
        });
        let s = &self.store.snapshot;
        self.panel_mode = Some((
            s.state,
            s.has_reading,
            s.insufficient,
            s.adapter_watts,
            s.adapter_volts,
            s.watts().unwrap_or(0.) > 0.,
            connected_draining(s),
        ));
        let _: () = msg_send![root, release];
        let _: () = msg_send![scroll, release];
        self.update();
        self.update_consumers();
        self.trace("rebuild_end");
    }
}
pub fn run(store: Store, options: Options) {
    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let app = NSApp();
        app.setActivationPolicy_(NSApplicationActivationPolicyAccessory);
        let controller_class = register();
        let target: id = msg_send![controller_class, new];
        let status_bar: id = msg_send![class!(NSStatusBar), systemStatusBar];
        let status: id = msg_send![status_bar,statusItemWithLength:-1.0_f64];
        let _: () = msg_send![status, retain];
        let button: id = msg_send![status, button];
        let _: () = msg_send![button,setTarget:target];
        let _: () = msg_send![button,setAction:sel!(togglePanel:)];
        let _: usize = msg_send![button,sendActionOn:(1_u64<<2)|(1_u64<<4)];
        let _: () = msg_send![button,setFont:font(12.,true)];
        let _: () = msg_send![button,setImagePosition:2_usize];
        let _: () = msg_send![button,setAccessibilityLabel:string("BatteryBar")];
        let popover: id = msg_send![class!(NSPopover), new];
        let _: () = msg_send![popover,setBehavior:if options.hold_panel{0_usize}else{1_usize}];
        let _: () = msg_send![popover,setAnimates:YES];
        let start = model::now();
        let mut a = App {
            store,
            status,
            popover,
            target,
            panel: None,
            report: Report::default(),
            show_settings: options.settings_open,
            show_all: false,
            last_collect: start,
            settle_until: 0.,
            last_settle: 0.,
            start,
            options,
            bar_text: "\0".into(),
            bar_image_key: (-1, Power::Discharging, false, false, false),
            panel_mode: None,
            memory_samples: Vec::new(),
            last_memory: 0.,
            cycle_step: 0,
            ui_checks: Vec::new(),
            pending_rebuild: false,
            pending_report: false,
            regression: None,
            lifecycle: Vec::new(),
            sweep_step: 0,
            not_before_quit: 0.,
        };
        a.store.refresh();
        if a.options.expanded_cycles {
            a.store
                .settings
                .set("activity_history_hours", serde_json::json!(6));
            a.store
                .settings
                .set("activity_group_apps", serde_json::json!(false));
        }
        a.store.collect();
        a.update();
        APP.with(|cell| *cell.borrow_mut() = Some(a));
        platform::events(event);
        let timer: id = msg_send![class!(NSTimer),timerWithTimeInterval:1.0_f64 target:target selector:sel!(tick:) userInfo:nil repeats:YES];
        let runloop: id = msg_send![class!(NSRunLoop), mainRunLoop];
        let _: () = msg_send![runloop,addTimer:timer forMode:string("kCFRunLoopCommonModes")];
        app.run();
        platform::stop_events();
    }
}
struct Pool(id);
unsafe fn invoke_control(tag: i64, segment: Option<isize>) -> bool {
    let _pool = Pool::new();
    let control = APP.with(|cell| {
        let b = cell.borrow();
        let Some(p) = b.as_ref().and_then(|a| a.panel.as_ref()) else {
            return nil;
        };
        let children: id = msg_send![p.root, subviews];
        let count: usize = msg_send![children, count];
        for i in 0..count {
            let child: id = msg_send![children,objectAtIndex:i];
            let child_tag: i64 = msg_send![child, tag];
            if child_tag == tag {
                let _: () = msg_send![child, retain];
                return child;
            }
        }
        nil
    });
    if control == nil {
        return false;
    }
    if let Some(i) = segment {
        let _: () = msg_send![control,setSelectedSegment:i];
    }
    let target: id = msg_send![control, target];
    let selector: Sel = msg_send![control, action];
    let sent: bool = msg_send![NSApp(),sendAction:selector to:target from:control];
    let _: () = msg_send![control, release];
    sent
}
unsafe fn exercise_ui() -> Vec<serde_json::Value> {
    let mut checks = Vec::new();
    let mut record =
        |name: String, pass: bool| checks.push(serde_json::json!({"check":name,"pass":pass}));
    record(
        "refresh".into(),
        invoke_control(1, None)
            && APP.with(|c| c.borrow().as_ref().unwrap().store.snapshot.has_reading),
    );
    for (i, hours) in [1, 6, 12, 24, 48].into_iter().enumerate() {
        let sent = invoke_control(10, Some(i as isize));
        record(
            format!("period_{hours}h"),
            sent && APP.with(|c| {
                c.borrow()
                    .as_ref()
                    .unwrap()
                    .store
                    .settings
                    .int("activity_history_hours")
                    == hours
            }),
        );
    }
    for i in 0..=1 {
        let sent = invoke_control(11, Some(i));
        record(
            format!("group_{i}"),
            sent && APP.with(|c| {
                c.borrow()
                    .as_ref()
                    .unwrap()
                    .store
                    .settings
                    .flag("activity_group_apps")
                    == (i == 0)
            }),
        );
    }
    for (key, tag) in [
        ("bar_compact", 20),
        ("bar_percentage", 21),
        ("bar_time", 22),
        ("bar_wattage", 23),
        ("notify_full", 24),
        ("notify_low", 25),
        ("notify_limit", 26),
        ("track_history", 27),
    ] {
        let before = APP.with(|c| c.borrow().as_ref().unwrap().store.settings.flag(key));
        let sent = invoke_control(tag, None);
        let changed = APP.with(|c| c.borrow().as_ref().unwrap().store.settings.flag(key) != before);
        let restored = invoke_control(tag, None)
            && APP.with(|c| c.borrow().as_ref().unwrap().store.settings.flag(key) == before);
        record(key.into(), sent && changed && restored);
    }
    for (key, tag, values) in [
        ("notify_low_pct", 30, [10, 15, 20, 30]),
        ("notify_limit_pct", 31, [70, 80, 85, 90]),
    ] {
        for (i, value) in values.into_iter().enumerate() {
            let sent = invoke_control(tag, Some(i as isize));
            record(
                format!("{key}_{value}"),
                sent && APP.with(|c| c.borrow().as_ref().unwrap().store.settings.int(key) == value),
            );
        }
    }
    let before = APP.with(|c| c.borrow().as_ref().unwrap().show_all);
    let sent = invoke_control(4, None);
    record(
        "show_all".into(),
        sent && APP.with(|c| c.borrow().as_ref().unwrap().show_all != before),
    );
    invoke_control(4, None);
    let before = platform::login();
    let sent = invoke_control(28, None);
    record(
        "login_isolated_guard".into(),
        sent && platform::login() == before,
    );
    let sent = invoke_control(3, None);
    record(
        "settings_close".into(),
        sent && APP.with(|c| !c.borrow().as_ref().unwrap().show_settings),
    );
    let sent = invoke_control(3, None);
    record(
        "settings_open".into(),
        sent && APP.with(|c| c.borrow().as_ref().unwrap().show_settings),
    );
    let appearance_test = APP.with(|c| {
        let b = c.borrow();
        let a = b.as_ref().unwrap();
        let button: id = msg_send![a.status, button];
        a.options
            .capture
            .as_ref()
            .and_then(|p| p.parent())
            .map(|dir| {
                (
                    button,
                    dir.join("status-appearance").to_string_lossy().into_owned(),
                )
            })
    });
    if let Some((button, prefix)) = appearance_test {
        checks.extend(platform::status_appearance_check(button.cast(), &prefix));
    }
    checks
}
unsafe fn context_menu(status: id, target: id) {
    // Menu tracking runs a nested event loop; do not hold the model RefCell borrow.
    let menu: id = msg_send![class!(NSMenu), new];
    for (title, tag, key) in [("Aggiorna adesso", 1_i64, "r"), ("Esci", 2_i64, "q")] {
        let item: id = msg_send![class!(NSMenuItem), alloc];
        let item: id = msg_send![item,initWithTitle:string(title) action:sel!(action:) keyEquivalent:string(key)];
        let _: () = msg_send![item,setTarget:target];
        let _: () = msg_send![item,setTag:tag];
        let _: () = msg_send![menu,addItem:item];
        let _: () = msg_send![item, release];
    }
    let _: () = msg_send![status,setMenu:menu];
    let button: id = msg_send![status, button];
    let _: () = msg_send![button,performClick:nil];
    let _: () = msg_send![status,setMenu:nil];
    let _: () = msg_send![menu, release];
}
impl Pool {
    unsafe fn new() -> Self {
        Self(NSAutoreleasePool::new(nil))
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        unsafe {
            self.0.drain();
        }
    }
}

#[cfg(test)]
mod power_presentation_tests {
    use super::*;

    #[test]
    fn distinguishes_connected_drain_from_real_charging() {
        let mut s = model::Snapshot {
            state: Power::Charging,
            has_reading: true,
            voltage: Some(12.0),
            ma: Some(-500),
            ..Default::default()
        };
        assert_eq!(visible_power_state(&s), "Batteria in carica");
        s.insufficient = true;
        assert_eq!(
            visible_power_state(&s),
            "Alimentatore collegato · batteria in scarica"
        );
        assert_eq!(power_symbol(&s), "arrow.down.circle.fill");
        assert_eq!(estimate_heading(&s), "TEMPO ALLA SCARICA");
        assert_eq!(tint(&s), (0.94, 0.58, 0.21));
        s.insufficient = false;
        s.ma = Some(500);
        assert_eq!(visible_power_state(&s), "Batteria in carica");
        assert_eq!(power_symbol(&s), "bolt.fill");
        assert_eq!(estimate_heading(&s), "TEMPO ALLA CARICA COMPLETA");
        s.ma = None;
        assert_eq!(visible_power_state(&s), "Batteria in carica");
        assert_eq!(power_symbol(&s), "bolt.fill");
        s.stale = true;
        assert_eq!(visible_power_state(&s), "Stato batteria non disponibile");
    }
}
