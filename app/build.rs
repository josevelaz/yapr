fn main() {
    // ScreenCaptureKit's Swift bridge uses macOS's shared-cache concurrency runtime.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
