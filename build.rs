fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("failed to compile Slint UI");

    // Icon, version information and manifest of the Windows executable.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        println!("cargo:rerun-if-changed=assets/app.manifest");
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icon.ico");
        resource.set_manifest_file("assets/app.manifest");
        resource.set_language(0x0411); // Japanese (Japan)
        if let Err(err) = resource.compile() {
            // Not fatal: the program works without the embedded resources.
            println!("cargo:warning=failed to embed Windows resources: {err}");
        }
    }
}
