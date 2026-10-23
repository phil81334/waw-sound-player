fn main() {
    println!("cargo:rerun-if-changed=native/audio.cpp");
    cc::Build::new()
        .cpp(true)
        .file("native/audio.cpp")
        .flag_if_supported("/std:c++17")
        .compile("waw_audio");
    println!("cargo:rustc-link-lib=xaudio2");
    println!("cargo:rustc-link-lib=ole32");
    tauri_build::build();
}
