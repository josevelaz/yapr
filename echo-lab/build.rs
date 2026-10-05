fn main() {
    // ScreenCaptureKit's Swift bridge links @rpath/libswift_Concurrency.dylib.
    // macOS provides it in the shared cache under /usr/lib/swift.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}
