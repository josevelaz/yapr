fn main() {
    // ScreenCaptureKit's Swift bridge uses macOS's shared-cache concurrency runtime.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    println!("cargo:rerun-if-changed=build.rs");

    // Optional release secrets. An unset or blank value leaves the feature out of the build.
    for name in [
        "YAPR_GOOGLE_CLIENT_ID",
        "YAPR_GOOGLE_CLIENT_SECRET",
        "YAPR_LIATRIO_GATEWAY_KEY",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
        let value = std::env::var(name).unwrap_or_default();
        let value = value.trim();
        if !value.is_empty() {
            let embedded = name.replacen("YAPR_", "YAPR_EMBEDDED_", 1);
            println!("cargo:rustc-env={embedded}={value}");
        }
    }
}
