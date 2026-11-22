//! The native UI (lane native-ui): compiles the .slint files under `ui/` to
//! Rust, embeds the Common Controls manifest in every linked target, and puts
//! the icon and version info into the exe, which tauri-build used to do.

/// voice lane: transcribe.cpp has to be built with src/voice/whisper.cmake
/// (the static C++ runtime, AVX2 and no AVX-512). Left to itself the library
/// tunes itself to the build machine's processor and the exe can crash on an
/// older PC. The flags reach its CMake build through TRANSCRIBE_CMAKE_ARGS,
/// an absolute path cargo's own [env] table cannot write, so the workflows
/// set it (tools/voice-cmake-env.mjs) and a build without it stops here.
fn voice_flags() {
    println!("cargo:rerun-if-env-changed=TRANSCRIBE_CMAKE_ARGS");
    println!("cargo:rerun-if-env-changed=TRANSCRIBE_DIR");
    let args = std::env::var("TRANSCRIBE_CMAKE_ARGS").unwrap_or_default();
    let prebuilt = std::env::var_os("TRANSCRIBE_DIR").is_some();
    assert!(
        prebuilt || args.contains("voice/whisper.cmake"),
        "TRANSCRIBE_CMAKE_ARGS does not name src/voice/whisper.cmake: \
         run `node tools/voice-cmake-env.mjs` and set the variable it prints"
    );
}

fn main() {
    voice_flags();
    // The Common Controls v6 manifest goes to every linked target, unit
    // tests included: without it the test exe dies on load (0xc0000139,
    // TaskDialogIndirect). It also turns on per-monitor DPI awareness v2.
    let manifest = std::env::current_dir().expect("cwd").join("app.manifest");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());

    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("compile the .slint UI");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icons/icon.ico");
        res.set("ProductName", "Aevaleron");
        res.set("FileDescription", "Aevaleron");
        res.set("CompanyName", "darkelights");
        res.set("OriginalFilename", "Aevaleron.exe");
        res.compile().expect("embed the icon and version info");
    }
    println!("cargo:rerun-if-changed=icons/icon.ico");
}
