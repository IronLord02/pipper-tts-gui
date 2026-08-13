fn main() {
    tauri_build::build();

    // Tauri embeds the application manifest (Common-Controls v6 dependency)
    // only into binary targets — `embed-resource` emits `rustc-link-arg-bins`.
    // Test binaries therefore bind comctl32 v5 from system32, which lacks
    // `TaskDialogIndirect` (used by tauri-runtime-wry dialogs), and die at load
    // with STATUS_ENTRYPOINT_NOT_FOUND. Emit the same manifest to test targets
    // so `cargo test` runs on Windows.
    #[cfg(target_os = "windows")]
    {
        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());

        let manifest = out_dir.join("app-manifest-test.xml");
        std::fs::write(
            &manifest,
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#,
        )
        .expect("write test manifest");

        let rc = out_dir.join("app-manifest-test.rc");
        std::fs::write(&rc, "1 24 \"app-manifest-test.xml\"\n").expect("write test rc");

        embed_resource::compile_for_everything(&rc, embed_resource::NONE)
            .manifest_required()
            .expect("embed test manifest");

        // The WebView2 loader DLL is copied next to binary outputs by
        // tauri-build, but test executables live in `<target>/debug/deps`
        // where nothing places it. A stale loader in PATH would otherwise be
        // picked up at load time. Copy the loader produced by webview2-com-sys
        // next to the test executables.
        let deps_dir = out_dir
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.join("deps"));
        if let Some(deps_dir) = deps_dir {
            let target_dir = out_dir
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .and_then(|p| p.parent())
                .expect("target dir");
            let build_root = target_dir.join("debug").join("build");
            if let Ok(entries) = std::fs::read_dir(&build_root) {
                for entry in entries.flatten() {
                    let loader = entry
                        .path()
                        .join("out")
                        .join("x64")
                        .join("WebView2Loader.dll");
                    if loader.is_file() {
                        let dest = deps_dir.join("WebView2Loader.dll");
                        let _ = std::fs::copy(&loader, &dest);
                        break;
                    }
                }
            }
        }
    }
}