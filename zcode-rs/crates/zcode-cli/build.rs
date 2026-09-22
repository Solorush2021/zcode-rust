use std::process::Command;

fn main() {
    // Version comes from the TS CLI package.json so both binaries report identical versions.
    let out = Command::new("node")
        .arg("-p")
        .arg("require('/Users/vipulkumar/Documents/zai/zcode-port/ZCode/apps/zcode-cli/package.json').version")
        .output();
    let version = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "0.16.9".to_string(),
    };
    println!("cargo:rustc-env=ZCODE_VERSION={version}");
    println!("cargo:rustc-env=ZCODE_PORT_DIR_DEFAULT=/Users/vipulkumar/Documents/zai/zcode-port");
    if let Ok(rv) = std::env::var("ZCODE_RUSTC_VERSION") {
        println!("cargo:rustc-env=ZCODE_RUSTC_VERSION={rv}");
    } else {
        println!("cargo:rustc-env=ZCODE_RUSTC_VERSION=1.98.1");
    }
    println!("cargo:rerun-if-changed=src/help_template.txt");
}
