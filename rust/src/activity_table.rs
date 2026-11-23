//! Cell-based AppKit table: one NSCell per column and visible-row drawing.
//! The panel keeps one user scroll range. A clipped native viewport bounds raster
//! allocation; its wheel events and row navigation route to the outer scroll view.
#![allow(deprecated, unexpected_cfgs)]
use crate::{history::Row, model};
use cocoa::{
    base::{id, nil, NO, YES},
    foundation::{NSAutoreleasePool, NSPoint, NSRange, NSRect, NSSize, NSString},
};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Object, Protocol, Sel},
    sel, sel_impl,
};
use std::{
    ffi::{c_void, CStr},
    sync::atomic::{AtomicUsize, Ordering},
};

static ACTIVE_TABLES: AtomicUsize = AtomicUsize::new(0);
static CREATED_TABLES: AtomicUsize = AtomicUsize::new(0);
static DESTROYED_TABLES: AtomicUsize = AtomicUsize::new(0);
static PEAK_TABLES: AtomicUsize = AtomicUsize::new(0);
static RETAINED_ROWS: AtomicUsize = AtomicUsize::new(0);
static VALUE_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static DRAW_PASSES: AtomicUsize = AtomicUsize::new(0);
static MAX_VALUES_PER_DRAW: AtomicUsize = AtomicUsize::new(0);
static MAX_VISIBLE_ROWS: AtomicUsize = AtomicUsize::new(0);
static MAX_VIEWPORT_HEIGHT: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" {
    static NSViewBoundsDidChangeNotification: id;
}
struct TableData {
    rows: Vec<Row>,
}
unsafe fn data(this: &Object) -> Option<&TableData> {
    let p = *this.get_ivar::<*mut c_void>("rustRows");
    p.cast::<TableData>().as_ref()
}
unsafe fn string(s: &str) -> id {
    NSString::alloc(nil).init_str(s).autorelease()
}
unsafe fn identifier(column: id) -> String {
    let value: id = msg_send![column, identifier];
    let p: *const libc::c_char = msg_send![value, UTF8String];
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}
fn value(row: &Row, column: &str) -> String {
    match column {
        "cpu" => format!("{} CPU", model::activity_duration(row.cpu)),
        "energy" => row.joules.map(model::energy).unwrap_or("n/d".into()),
        _ => row.name().into(),
    }
}
extern "C" fn count(this: &Object, _: Sel, _: id) -> isize {
    unsafe { data(this).map_or(0, |d| d.rows.len() as isize) }
}
extern "C" fn object_value(this: &Object, _: Sel, _: id, column: id, row: isize) -> id {
    unsafe {
        VALUE_REQUESTS.fetch_add(1, Ordering::Relaxed);
        data(this)
            .and_then(|d| d.rows.get(row as usize))
            .map_or(nil, |r| string(&value(r, &identifier(column))))
    }
}
extern "C" fn tooltip(
    this: &Object,
    _: Sel,
    _: id,
    _: id,
    _: *mut c_void,
    _: id,
    row: isize,
    _: cocoa::foundation::NSPoint,
) -> id {
    unsafe {
        data(this)
            .and_then(|d| d.rows.get(row as usize))
            .map_or(nil, |r| string(&r.path))
    }
}
extern "C" fn should_select(_: &Object, _: Sel, _: id, _: isize) -> bool {
    NO
}
extern "C" fn draw(this: &Object, _: Sel, dirty: NSRect) {
    unsafe {
        let before = VALUE_REQUESTS.load(Ordering::Relaxed);
        let visible: NSRect = msg_send![this, visibleRect];
        let rows: NSRange = msg_send![this,rowsInRect:visible];
        if rows.length < u64::MAX / 2 {
            MAX_VISIBLE_ROWS.fetch_max(rows.length as usize, Ordering::Relaxed);
        }
        let _: () = msg_send![super(this,class!(NSTableView)),drawRect:dirty];
        MAX_VALUES_PER_DRAW.fetch_max(
            VALUE_REQUESTS.load(Ordering::Relaxed) - before,
            Ordering::Relaxed,
        );
        DRAW_PASSES.fetch_add(1, Ordering::Relaxed);
    }
}
extern "C" fn dealloc(this: &mut Object, _: Sel) {
    unsafe {
        let _: () = msg_send![this,setDataSource:nil];
        let _: () = msg_send![this,setDelegate:nil];
        let p = *this.get_ivar::<*mut c_void>("rustRows");
        if !p.is_null() {
            let data = Box::from_raw(p.cast::<TableData>());
            RETAINED_ROWS.fetch_sub(data.rows.len(), Ordering::Relaxed);
            drop(data);
            this.set_ivar("rustRows", std::ptr::null_mut::<c_void>());
            ACTIVE_TABLES.fetch_sub(1, Ordering::Relaxed);
            DESTROYED_TABLES.fetch_add(1, Ordering::Relaxed);
        }
        let _: () = msg_send![super(this, class!(NSTableView)), dealloc];
    }
}
pub unsafe fn register() {
    let mut class = ClassDecl::new("BatteryBarRustActivityTable", class!(NSTableView)).unwrap();
    class.add_ivar::<*mut c_void>("rustRows");
    for name in ["NSTableViewDataSource", "NSTableViewDelegate"] {
        if let Some(p) = Protocol::get(name) {
            class.add_protocol(p);
        }
    }
    class.add_method(
        sel!(numberOfRowsInTableView:),
        count as extern "C" fn(&Object, Sel, id) -> isize,
    );
    class.add_method(
        sel!(tableView:objectValueForTableColumn:row:),
        object_value as extern "C" fn(&Object, Sel, id, id, isize) -> id,
    );
    class.add_method(
        sel!(tableView:toolTipForCell:rect:tableColumn:row:mouseLocation:),
        tooltip
            as extern "C" fn(
                &Object,
                Sel,
                id,
                id,
                *mut c_void,
                id,
                isize,
                cocoa::foundation::NSPoint,
            ) -> id,
    );
    class.add_method(
        sel!(tableView:shouldSelectRow:),
        should_select as extern "C" fn(&Object, Sel, id, isize) -> bool,
    );
    class.add_method(sel!(drawRect:), draw as extern "C" fn(&Object, Sel, NSRect));
    class.add_method(
        sel!(scrollRowToVisible:),
        scroll_row as extern "C" fn(&Object, Sel, isize),
    );
    class.add_method(
        sel!(scrollWheel:),
        table_wheel as extern "C" fn(&Object, Sel, id),
    );
    class.add_method(sel!(dealloc), dealloc as extern "C" fn(&mut Object, Sel));
    class.register();
    let mut viewport = ClassDecl::new("BatteryBarRustActivityViewport", class!(NSView)).unwrap();
    viewport.add_ivar::<id>("rustInnerScroll");
    viewport.add_ivar::<id>("rustOuterClip");
    viewport.add_ivar::<bool>("rustSyncing");
    viewport.add_method(
        sel!(isFlipped),
        flipped as extern "C" fn(&Object, Sel) -> bool,
    );
    viewport.add_method(
        sel!(tag),
        viewport_tag as extern "C" fn(&Object, Sel) -> isize,
    );
    viewport.add_method(
        sel!(wantsUpdateLayer),
        flipped as extern "C" fn(&Object, Sel) -> bool,
    );
    viewport.add_method(
        sel!(updateLayer),
        empty_layer as extern "C" fn(&Object, Sel),
    );
    viewport.add_method(
        sel!(outerScrolled:),
        outer_scrolled as extern "C" fn(&Object, Sel, id),
    );
    viewport.add_method(
        sel!(dealloc),
        viewport_dealloc as extern "C" fn(&mut Object, Sel),
    );
    viewport.register();
    let mut scroll = ClassDecl::new("BatteryBarRustActivityScroll", class!(NSScrollView)).unwrap();
    scroll.add_method(
        sel!(scrollWheel:),
        inner_wheel as extern "C" fn(&Object, Sel, id),
    );
    scroll.register();
}
unsafe fn create_table(rows: &[Row], frame: NSRect) -> id {
    let table: id = msg_send![class!(BatteryBarRustActivityTable), alloc];
    let table: id = msg_send![table,initWithFrame:frame];
    let owned = Box::new(TableData {
        rows: rows.to_vec(),
    });
    RETAINED_ROWS.fetch_add(rows.len(), Ordering::Relaxed);
    let active = ACTIVE_TABLES.fetch_add(1, Ordering::Relaxed) + 1;
    CREATED_TABLES.fetch_add(1, Ordering::Relaxed);
    PEAK_TABLES.fetch_max(active, Ordering::Relaxed);
    (*table).set_ivar("rustRows", Box::into_raw(owned).cast::<c_void>());
    let _: () = msg_send![table,setTag:40_i64];
    let _: () = msg_send![table,setHeaderView:nil];
    let _: () = msg_send![table,setCornerView:nil];
    let _: () = msg_send![table,setRowHeight:23.0_f64];
    let _: () = msg_send![table,setRowSizeStyle:0_isize];
    let _: () = msg_send![table,setUsesAutomaticRowHeights:NO];
    let _: () = msg_send![table,setStyle:1_isize];
    let _: () = msg_send![table,setIntercellSpacing:NSSize::new(0.,0.)];
    let _: () = msg_send![table,setColumnAutoresizingStyle:0_usize];
    let _: () = msg_send![table,setSelectionHighlightStyle:-1_isize];
    let _: () = msg_send![table,setAllowsEmptySelection:YES];
    let clear: id = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![table,setBackgroundColor:clear];
    let _: () = msg_send![table,setUsesAlternatingRowBackgroundColors:NO];
    let _: () = msg_send![table,setAllowsColumnReordering:NO];
    let _: () = msg_send![table,setAllowsColumnResizing:NO];
    let font: id = msg_send![class!(NSFont),systemFontOfSize:10.0_f64];
    for (name, width, secondary) in [
        ("name", 174., false),
        ("cpu", 97., true),
        ("energy", 77., false),
    ] {
        let column: id = msg_send![class!(NSTableColumn), alloc];
        let column: id = msg_send![column,initWithIdentifier:string(name)];
        let _: () = msg_send![column,setWidth:width];
        let _: () = msg_send![column,setMinWidth:width];
        let _: () = msg_send![column,setMaxWidth:width];
        let _: () = msg_send![column,setEditable:NO];
        let cell: id = msg_send![class!(NSTextFieldCell), alloc];
        let cell: id = msg_send![cell,initTextCell:string("")];
        let _: () = msg_send![cell,setFont:font];
        let _: () = msg_send![cell,setLineBreakMode:4_usize];
        let _: () = msg_send![cell,setUsesSingleLineMode:YES];
        let _: () = msg_send![cell,setEditable:NO];
        let c: id = if secondary {
            msg_send![class!(NSColor), secondaryLabelColor]
        } else {
            msg_send![class!(NSColor), labelColor]
        };
        let _: () = msg_send![cell,setTextColor:c];
        let _: () = msg_send![column,setDataCell:cell];
        let _: () = msg_send![cell, release];
        let _: () = msg_send![table,addTableColumn:column];
        let _: () = msg_send![column, release];
    }
    let _: () = msg_send![table,setDataSource:table];
    let _: () = msg_send![table,setDelegate:table];
    let _: () = msg_send![table, reloadData];
    table
}
pub unsafe fn create(parent: id, rows: &[Row], frame: NSRect) -> id {
    // A transparent logical extent reserves the original continuous scroll range.
    // Only the intersection with the outer clip is backed by a real scroll viewport.
    let viewport: id = msg_send![class!(BatteryBarRustActivityViewport), alloc];
    let viewport: id = msg_send![viewport,initWithFrame:frame];
    let _: () = msg_send![viewport,setWantsLayer:YES];
    let inner: id = msg_send![class!(BatteryBarRustActivityScroll), alloc];
    let inner: id = msg_send![inner,initWithFrame:NSRect::new(NSPoint::new(0.,0.),NSSize::new(frame.size.width,0.))];
    let _: () = msg_send![inner,setHasVerticalScroller:NO];
    let _: () = msg_send![inner,setHasHorizontalScroller:NO];
    let _: () = msg_send![inner,setDrawsBackground:NO];
    let _: () = msg_send![inner,setBorderType:0_usize];
    let _: () = msg_send![inner,setAutomaticallyAdjustsContentInsets:NO];
    let table = create_table(rows, NSRect::new(NSPoint::new(0., 0.), frame.size));
    let _: () = msg_send![inner,setDocumentView:table];
    let _: () = msg_send![table, release];
    let _: () = msg_send![viewport,addSubview:inner];
    (*viewport).set_ivar("rustInnerScroll", inner);
    let _: () = msg_send![inner, release];
    let _: () = msg_send![parent,addSubview:viewport];
    viewport
}
unsafe fn table(viewport: id) -> id {
    let inner = *(*viewport).get_ivar::<id>("rustInnerScroll");
    msg_send![inner, documentView]
}
pub unsafe fn focus_target(viewport: id) -> id {
    table(viewport)
}
pub unsafe fn update(viewport: id, rows: &[Row]) {
    let table = table(viewport);
    let p = *(*table).get_ivar::<*mut c_void>("rustRows");
    let old = Box::from_raw(p.cast::<TableData>());
    RETAINED_ROWS.fetch_sub(old.rows.len(), Ordering::Relaxed);
    let new = Box::new(TableData {
        rows: rows.to_vec(),
    });
    RETAINED_ROWS.fetch_add(rows.len(), Ordering::Relaxed);
    (*table).set_ivar("rustRows", Box::into_raw(new).cast::<c_void>());
    drop(old);
    let frame: NSRect = msg_send![table, frame];
    let _: () = msg_send![table,setFrameSize:NSSize::new(frame.size.width,rows.len() as f64*23.)];
    let _: () = msg_send![table, reloadData];
    let _: () = msg_send![table,setNeedsDisplay:YES];
    sync(viewport);
}
extern "C" fn flipped(_: &Object, _: Sel) -> bool {
    YES
}
extern "C" fn viewport_tag(_: &Object, _: Sel) -> isize {
    40
}
extern "C" fn empty_layer(_: &Object, _: Sel) {}
extern "C" fn outer_scrolled(this: &Object, _: Sel, _: id) {
    unsafe {
        sync(this as *const Object as id);
    }
}
extern "C" fn viewport_dealloc(this: &mut Object, _: Sel) {
    unsafe {
        detach(this as *mut Object);
        let _: () = msg_send![super(this, class!(NSView)), dealloc];
    }
}
unsafe fn forward(viewport: id, event: id) {
    let clip = *(*viewport).get_ivar::<id>("rustOuterClip");
    if clip != nil {
        let outer: id = msg_send![clip, enclosingScrollView];
        let _: () = msg_send![outer,scrollWheel:event];
    }
}
extern "C" fn inner_wheel(this: &Object, _: Sel, event: id) {
    unsafe {
        let parent: id = msg_send![this, superview];
        forward(parent, event);
    }
}
extern "C" fn table_wheel(this: &Object, _: Sel, event: id) {
    unsafe {
        let inner: id = msg_send![this, enclosingScrollView];
        let parent: id = msg_send![inner, superview];
        forward(parent, event);
    }
}
extern "C" fn scroll_row(this: &Object, _: Sel, row: isize) {
    unsafe {
        let inner: id = msg_send![this, enclosingScrollView];
        let parent: id = msg_send![inner, superview];
        let frame: NSRect = msg_send![this, frame];
        let _: bool = msg_send![parent,scrollRectToVisible:NSRect::new(NSPoint::new(0.,row.max(0) as f64*23.),NSSize::new(frame.size.width,23.))];
    }
}
pub unsafe fn attach(viewport: id, outer: id) {
    if viewport == nil {
        return;
    }
    detach(viewport);
    let clip: id = msg_send![outer, contentView];
    (*viewport).set_ivar("rustOuterClip", clip);
    let _: () = msg_send![clip,setPostsBoundsChangedNotifications:YES];
    let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
    let _: () = msg_send![center,addObserver:viewport selector:sel!(outerScrolled:) name:NSViewBoundsDidChangeNotification object:clip];
    sync(viewport);
}
pub unsafe fn detach(viewport: id) {
    if viewport == nil {
        return;
    }
    let clip = *(*viewport).get_ivar::<id>("rustOuterClip");
    if clip != nil {
        let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
        let _: () = msg_send![center,removeObserver:viewport name:NSViewBoundsDidChangeNotification object:clip];
        (*viewport).set_ivar("rustOuterClip", nil);
    }
}
pub unsafe fn sync(viewport: id) {
    if viewport == nil || *(*viewport).get_ivar::<bool>("rustSyncing") {
        return;
    }
    (*viewport).set_ivar("rustSyncing", true);
    let visible: NSRect = msg_send![viewport, visibleRect];
    let bounds: NSRect = msg_send![viewport, bounds];
    let inner = *(*viewport).get_ivar::<id>("rustInnerScroll");
    let height = visible.size.height.clamp(0., 620.);
    MAX_VIEWPORT_HEIGHT.fetch_max(height.ceil() as usize, Ordering::Relaxed);
    let _: () = msg_send![inner,setHidden:height<0.5];
    let _: () = msg_send![inner,setFrame:NSRect::new(NSPoint::new(0.,visible.origin.y),NSSize::new(bounds.size.width,height))];
    let clip: id = msg_send![inner, contentView];
    let _: () = msg_send![clip,scrollToPoint:NSPoint::new(0.,visible.origin.y)];
    let _: () = msg_send![inner,reflectScrolledClipView:clip];
    (*viewport).set_ivar("rustSyncing", false);
}
pub fn diagnostics() -> serde_json::Value {
    serde_json::json!({"active_tables":ACTIVE_TABLES.load(Ordering::Relaxed),"created_tables":CREATED_TABLES.load(Ordering::Relaxed),"destroyed_tables":DESTROYED_TABLES.load(Ordering::Relaxed),"peak_tables":PEAK_TABLES.load(Ordering::Relaxed),"retained_rows":RETAINED_ROWS.load(Ordering::Relaxed),"value_requests":VALUE_REQUESTS.load(Ordering::Relaxed),"draw_passes":DRAW_PASSES.load(Ordering::Relaxed),"max_values_per_draw":MAX_VALUES_PER_DRAW.load(Ordering::Relaxed),"max_visible_rows":MAX_VISIBLE_ROWS.load(Ordering::Relaxed),"max_viewport_height":MAX_VIEWPORT_HEIGHT.load(Ordering::Relaxed)})
}
pub unsafe fn validate_access(viewport: id, expected: &[Row]) -> serde_json::Value {
    let table = table(viewport);
    let count: isize = msg_send![table, numberOfRows];
    let columns: id = msg_send![table, tableColumns];
    let n: usize = msg_send![columns, count];
    let role: id = msg_send![table, accessibilityRole];
    let role_ptr: *const libc::c_char = msg_send![role, UTF8String];
    let role = if role_ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(role_ptr).to_string_lossy().into_owned()
    };
    let mut values_match = true;
    let mut tips_match = true;
    let mut geometry_match = true;
    let mut checked = 0;
    if !expected.is_empty() {
        for row in [0, expected.len() / 2, expected.len() - 1] {
            for (column, name) in ["name", "cpu", "energy"].iter().enumerate() {
                let column: id = msg_send![columns,objectAtIndex:column];
                let value: id = msg_send![table,tableView:table objectValueForTableColumn:column row:row as isize];
                let ptr: *const libc::c_char = msg_send![value, UTF8String];
                values_match &= !ptr.is_null()
                    && CStr::from_ptr(ptr).to_string_lossy() == self::value(&expected[row], name);
            }
            let column: id = msg_send![columns,objectAtIndex:0_usize];
            let tip: id = msg_send![table,tableView:table toolTipForCell:nil rect:std::ptr::null_mut::<c_void>() tableColumn:column row:row as isize mouseLocation:cocoa::foundation::NSPoint::new(0.,0.)];
            let ptr: *const libc::c_char = msg_send![tip, UTF8String];
            tips_match &=
                !ptr.is_null() && CStr::from_ptr(ptr).to_string_lossy() == expected[row].path;
            let actual: isize =
                msg_send![table,rowAtPoint:cocoa::foundation::NSPoint::new(10.,row as f64*23.+1.)];
            geometry_match &= actual == row as isize;
            checked += 1;
        }
    }
    let children: id = msg_send![table, subviews];
    let views: usize = msg_send![children, count];
    let outer_clip = *(*viewport).get_ivar::<id>("rustOuterClip");
    let before: NSRect = msg_send![outer_clip, bounds];
    let mut scroll_matches = true;
    if count > 0 {
        let _: () = msg_send![table,scrollRowToVisible:count-1];
        let visible: NSRect = msg_send![table, visibleRect];
        let last: NSRect = msg_send![table,rectOfRow:count-1];
        scroll_matches = visible.size.height > 0.
            && last.origin.y >= visible.origin.y - 1.
            && last.origin.y + last.size.height <= visible.origin.y + visible.size.height + 1.;
        let _: () = msg_send![outer_clip,scrollToPoint:before.origin];
        let outer: id = msg_send![outer_clip, enclosingScrollView];
        let _: () = msg_send![outer,reflectScrolledClipView:outer_clip];
        sync(viewport);
        let restored: NSRect = msg_send![outer_clip, bounds];
        scroll_matches &= (restored.origin.y - before.origin.y).abs() < 0.5;
    }
    let pass = count == expected.len() as isize
        && n == 3
        && role == "AXTable"
        && values_match
        && tips_match
        && geometry_match
        && scroll_matches;
    serde_json::json!({"check":"virtual_table_values_tooltips_accessibility_geometry","pass":pass,"rows":count,"columns":n,"checked_first_middle_last":checked,"row_subviews":views,"role":role,"values_match":values_match,"tooltips_match":tips_match,"geometry_match":geometry_match,"row_navigation_preserves_outer_scroll":scroll_matches})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_preserves_names_units_and_unknown_energy() {
        let row = Row {
            path: "/Apps/Test.app/Contents/MacOS/Test".into(),
            cpu: 120.,
            joules: Some(7200.),
        };
        assert_eq!(value(&row, "name"), "Test");
        assert_eq!(value(&row, "cpu"), "2.0 min CPU");
        assert_eq!(value(&row, "energy"), "2.000 Wh");
        assert_eq!(
            value(
                &Row {
                    joules: None,
                    ..row
                },
                "energy"
            ),
            "n/d"
        );
    }
}
