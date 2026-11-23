fn main() {
    cc::Build::new()
        .file("native/platform.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag("-Wno-deprecated-declarations")
        .compile("batterybar_platform");
    for framework in [
        "AppKit",
        "Foundation",
        "IOKit",
        "ServiceManagement",
        "UserNotifications",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rerun-if-changed=native/platform.m");
}
