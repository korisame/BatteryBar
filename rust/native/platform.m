#import <AppKit/AppKit.h>
#import <IOKit/IOKitLib.h>
#import <IOKit/ps/IOPowerSources.h>
#import <ServiceManagement/ServiceManagement.h>
#import <UserNotifications/UserNotifications.h>
#import <libproc.h>
#import <notify.h>
#import <mach/mach_time.h>
#import <sys/resource.h>

// Thin ownership boundary. All returned JSON is malloc-owned and freed in Rust.
static char *json(id object) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:object options:0 error:NULL];
    if (!data) return strdup("null");
    char *s = malloc(data.length+1); memcpy(s, data.bytes, data.length); s[data.length]=0; return s;
}
char *bb_battery(void) {
    @autoreleasepool {
        io_service_t service = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("AppleSmartBattery"));
        if (!service) return strdup("null");
        CFMutableDictionaryRef raw = NULL;
        kern_return_t result = IORegistryEntryCreateCFProperties(service, &raw, kCFAllocatorDefault, 0);
        IOObjectRelease(service);
        if (result || !raw) return strdup("null");
        NSDictionary *props = CFBridgingRelease(raw);
        NSMutableDictionary *d = [NSMutableDictionary dictionary];
        NSDictionary *battery = [props[@"BatteryData"] isKindOfClass:NSDictionary.class] ? props[@"BatteryData"] : @{};
        for (NSString *key in @[@"CurrentCapacity",@"AppleRawMaxCapacity",@"AppleRawCurrentCapacity",@"ExternalConnected",@"AppleRawExternalConnected",@"IsCharging",@"FullyCharged",@"Voltage",@"DesignCapacity",@"CycleCount",@"Temperature"]) {
            id value = [props[key] isKindOfClass:NSNumber.class] ? props[key] : battery[key];
            if ([value isKindOfClass:NSNumber.class]) d[key] = value;
        }
        NSDictionary *charger = props[@"ChargerData"];
        if ([charger isKindOfClass:NSDictionary.class] && [charger[@"IsCharging"] isKindOfClass:NSNumber.class])
            d[@"ControllerCharging"] = @([charger[@"IsCharging"] boolValue] ? YES : NO);
        if (props[@"InstantAmperage"]) d[@"InstantAmperage"] = @([props[@"InstantAmperage"] longLongValue]);
        NSDictionary *adapter = props[@"AdapterDetails"];
        if ([adapter isKindOfClass:NSDictionary.class]) {
            NSMutableDictionary *a = [NSMutableDictionary dictionary];
            for (NSString *key in @[@"Watts",@"AdapterVoltage"]) if ([adapter[key] isKindOfClass:NSNumber.class]) a[key] = adapter[key];
            d[@"AdapterDetails"] = a;
        }
        return json(d);
    }
}
double bb_timebase(void) { mach_timebase_info_data_t t; mach_timebase_info(&t); return (double)t.numer/t.denom; }
char *bb_counters(void) {
    @autoreleasepool {
        int capacity = MAX(4096, proc_listallpids(NULL,0)+1024);
        pid_t *pids = calloc(capacity,sizeof(pid_t));
        int count = proc_listallpids(pids,capacity*(int)sizeof(pid_t));
        NSMutableArray *rows = [NSMutableArray arrayWithCapacity:MAX(0,count)];
        double scale = bb_timebase()/1e9;
        for (int i=0;i<MIN(count,capacity);i++) {
            if (pids[i]<=0) continue;
            struct rusage_info_v6 r = {0};
            if (proc_pid_rusage(pids[i], RUSAGE_INFO_V6, (rusage_info_t *)&r)) continue;
            char path[PROC_PIDPATHINFO_MAXSIZE] = {0};
            if (proc_pidpath(pids[i],path,sizeof(path))<=0) continue;
            NSString *p = [NSString stringWithUTF8String:path]; if (!p) continue;
            [rows addObject:@{@"pid":@(pids[i]),@"started":@(r.ri_proc_start_abstime),@"path":p,
              @"cpu":@(((double)r.ri_user_time+(double)r.ri_system_time)*scale),
              @"energy":r.ri_energy_nj ? @((double)r.ri_energy_nj/1e9) : NSNull.null}];
        }
        free(pids); return json(rows);
    }
}
char *bb_devices(void) {
    @autoreleasepool {
        io_iterator_t it=0;
        if (IOServiceGetMatchingServices(kIOMainPortDefault,IOServiceMatching("IOUSBHostDevice"),&it)) return strdup("[]");
        NSMutableSet *names=[NSMutableSet set]; io_service_t device;
        while ((device=IOIteratorNext(it))) {
            id name=CFBridgingRelease(IORegistryEntryCreateCFProperty(device,CFSTR("USB Product Name"),kCFAllocatorDefault,0));
            if ([name isKindOfClass:NSString.class]) [names addObject:name];
            IOObjectRelease(device);
        }
        IOObjectRelease(it); return json([[names allObjects] sortedArrayUsingSelector:@selector(compare:)]);
    }
}
static NSUserDefaults *defaults;
char *bb_home_dir(void) { return strdup(NSHomeDirectory().UTF8String); }
void bb_preferences(bool isolated) {
    if (!isolated && [NSBundle.mainBundle.bundleIdentifier isEqualToString:@"com.shaungioelegori.batterybar"])
        defaults=NSUserDefaults.standardUserDefaults;
    else
        defaults=[[NSUserDefaults alloc] initWithSuiteName:isolated ? @"com.shaungioelegori.batterybar.rust-test" : @"com.shaungioelegori.batterybar"];
}
char *bb_get_setting(const char *key) {
    @autoreleasepool { id value=[defaults objectForKey:[NSString stringWithUTF8String:key]]; return json(@[value ?: NSNull.null]); }
}
void bb_set_setting(const char *key,const char *value) {
    @autoreleasepool {
        NSData *data=[[NSString stringWithUTF8String:value] dataUsingEncoding:NSUTF8StringEncoding];
        NSArray *a=[NSJSONSerialization JSONObjectWithData:data options:NSJSONReadingFragmentsAllowed error:NULL];
        if ([a isKindOfClass:NSArray.class] && a.count) [defaults setObject:a[0] forKey:[NSString stringWithUTF8String:key]];
    }
}
static bool notificationsEnabled;
void bb_notifications(bool enabled) {
    notificationsEnabled=enabled;
    if (enabled) [[UNUserNotificationCenter currentNotificationCenter] requestAuthorizationWithOptions:UNAuthorizationOptionAlert|UNAuthorizationOptionSound completionHandler:^(__unused BOOL ok,__unused NSError *error) {}];
}
void bb_notify(const char *title,const char *body) {
    if (!notificationsEnabled) return;
    @autoreleasepool {
        UNMutableNotificationContent *c=[UNMutableNotificationContent new]; c.title=[NSString stringWithUTF8String:title]; c.body=[NSString stringWithUTF8String:body]; c.sound=UNNotificationSound.defaultSound;
        [[UNUserNotificationCenter currentNotificationCenter] addNotificationRequest:[UNNotificationRequest requestWithIdentifier:NSUUID.UUID.UUIDString content:c trigger:nil] withCompletionHandler:nil];
    }
}
int bb_login_status(void) { return (int)SMAppService.mainAppService.status; }
char *bb_system_status(void) {
    @autoreleasepool {
        __block NSDictionary *notification=nil;
        dispatch_semaphore_t done=dispatch_semaphore_create(0);
        [[UNUserNotificationCenter currentNotificationCenter] getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *s) {
            notification=@{@"authorization":@(s.authorizationStatus),@"alert":@(s.alertSetting),@"sound":@(s.soundSetting)};
            dispatch_semaphore_signal(done);
        }];
        dispatch_semaphore_wait(done,dispatch_time(DISPATCH_TIME_NOW,5*NSEC_PER_SEC));
        return json(@{@"bundle_id":NSBundle.mainBundle.bundleIdentifier ?: @"",@"login_status":@(SMAppService.mainAppService.status),@"notification":notification ?: NSNull.null});
    }
}
bool bb_production_bundle(void) { return [NSBundle.mainBundle.bundleIdentifier isEqualToString:@"com.shaungioelegori.batterybar"]; }
char *bb_set_login(bool enabled) {
    @autoreleasepool { NSError *e=nil; if (enabled) [SMAppService.mainAppService registerAndReturnError:&e]; else [SMAppService.mainAppService unregisterAndReturnError:&e]; return e ? strdup(e.localizedDescription.UTF8String) : NULL; }
}
// Event callbacks stay on the main runloop, including the power settling burst.
static void (*eventCallback)(int);
static CFRunLoopSourceRef source;
static id wakeToken, outsideToken, resignToken;
static uint64_t changeGeneration;
static int sourceToken = NOTIFY_TOKEN_INVALID;
static void changed(__unused void *context) {
    if(eventCallback) eventCallback(1);
    uint64_t generation=++changeGeneration;
    for(int i=1;i<=8;i++) dispatch_after(dispatch_time(DISPATCH_TIME_NOW,(int64_t)i*250*NSEC_PER_MSEC),dispatch_get_main_queue(),^{
        if(eventCallback && generation==changeGeneration) eventCallback(4);
    });
}
void bb_events(void (*callback)(int)) {
    eventCallback=callback;
    notify_register_dispatch(kIOPSNotifyPowerSource, &sourceToken, dispatch_get_main_queue(), ^(int token){ if (token == sourceToken) changed(NULL); });
    source=IOPSNotificationCreateRunLoopSource(changed,NULL);
    if(source) CFRunLoopAddSource(CFRunLoopGetMain(),source,kCFRunLoopCommonModes);
    wakeToken=[NSWorkspace.sharedWorkspace.notificationCenter addObserverForName:NSWorkspaceDidWakeNotification object:nil queue:NSOperationQueue.mainQueue usingBlock:^(__unused NSNotification *n){if(eventCallback) eventCallback(2);changed(NULL);}];
}
void bb_watch_dismiss(bool enabled) {
    if(outsideToken) [NSEvent removeMonitor:outsideToken]; outsideToken=nil;
    if(resignToken) [NSNotificationCenter.defaultCenter removeObserver:resignToken]; resignToken=nil;
    if(!enabled) return;
    outsideToken=[NSEvent addGlobalMonitorForEventsMatchingMask:NSEventMaskLeftMouseDown|NSEventMaskRightMouseDown|NSEventMaskOtherMouseDown handler:^(__unused NSEvent *e){if(eventCallback) eventCallback(3);}];
    resignToken=[NSNotificationCenter.defaultCenter addObserverForName:NSApplicationDidResignActiveNotification object:nil queue:NSOperationQueue.mainQueue usingBlock:^(__unused NSNotification *n){if(eventCallback) eventCallback(3);}];
}
void bb_stop_events(void) {
    bb_watch_dismiss(false);
    if (sourceToken != NOTIFY_TOKEN_INVALID) { notify_cancel(sourceToken); sourceToken = NOTIFY_TOKEN_INVALID; }
    if(source) {CFRunLoopRemoveSource(CFRunLoopGetMain(),source,kCFRunLoopCommonModes);CFRelease(source);source=NULL;}
    if(wakeToken) [NSWorkspace.sharedWorkspace.notificationCenter removeObserver:wakeToken]; wakeToken=nil; eventCallback=NULL;
}
char *bb_memory(void) {
    struct rusage_info_v4 r={0};
    if(proc_pid_rusage(getpid(),RUSAGE_INFO_V4,(rusage_info_t *)&r)) return strdup("{}");
    return json(@{@"pid":@(getpid()),@"physical_footprint":@(r.ri_phys_footprint),@"resident":@(r.ri_resident_size),@"peak_footprint":@(r.ri_lifetime_max_phys_footprint)});
}
bool bb_capture_view(void *raw,const char *path) {
    NSView *v=(__bridge NSView *)raw;
    NSBitmapImageRep *rep=[v bitmapImageRepForCachingDisplayInRect:v.bounds];
    [v cacheDisplayInRect:v.bounds toBitmapImageRep:rep];
    return [[rep representationUsingType:NSBitmapImageFileTypePNG properties:@{}] writeToFile:[NSString stringWithUTF8String:path] atomically:YES];
}
char *bb_status_appearance_check(void *raw,const char *prefix) {
    // Called only by the finite isolated UI exercise on the AppKit main thread.
    // The live button is borrowed; its original explicit appearance is restored.
    NSStatusBarButton *button=(__bridge NSStatusBarButton *)raw;
    NSAppearance *original=button.appearance;
    NSMutableArray *checks=[NSMutableArray array];
    BOOL native=[button isKindOfClass:NSStatusBarButton.class] && button.contentTintColor==nil;
    [checks addObject:@{@"check":@"status_native_foreground",@"pass":@(native),@"class":NSStringFromClass(button.class),@"tint_override":@(button.contentTintColor!=nil),@"font_points":@(button.font.pointSize)}];
    for(NSString *name in @[NSAppearanceNameAqua,NSAppearanceNameDarkAqua]) {
        NSAppearance *appearance=[NSAppearance appearanceNamed:name];
        button.appearance=appearance;
        NSString *path=[NSString stringWithFormat:@"%s-%@.png",prefix,[name isEqualToString:NSAppearanceNameAqua]?@"light":@"dark"];
        __block BOOL saved=NO;
        [appearance performAsCurrentDrawingAppearance:^{
            [button setNeedsDisplay:YES];
            [button displayIfNeeded];
            saved=bb_capture_view((__bridge void *)button,path.UTF8String);
        }];
        BOOL matched=[[button.effectiveAppearance bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua,NSAppearanceNameDarkAqua]] isEqualToString:name];
        [checks addObject:@{@"check":[@"status_appearance_" stringByAppendingString:name],@"pass":@(native && matched && saved),@"effective_appearance":button.effectiveAppearance.name,@"capture":path}];
    }
    button.appearance=original;
    [button setNeedsDisplay:YES];
    return json(checks);
}
