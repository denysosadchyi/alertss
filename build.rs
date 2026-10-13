use embed_manifest::manifest::DpiAwareness;
use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    // MinGW's gcc already links its own default manifest (Win7-10 supportedOS, which layered
    // child windows need); a second one breaks the .rsrc merge. DPI awareness is also set at
    // runtime in main(), so on GNU we rely on that.
    let gnu = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() && !gnu {
        embed_manifest(
            new_manifest("SirenBar.Widget").dpi_awareness(DpiAwareness::PerMonitorV2),
        )
        .expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
