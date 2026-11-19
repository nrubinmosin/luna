//! `luna_test` for the test build (LUNA_FLAVOR=test, as the app's own build
//! script reads it): the driver a smoke run types through (src/drive.rs) is
//! compiled into it alone, never into a release.

fn main() {
    println!("cargo:rustc-check-cfg=cfg(luna_test)");
    println!("cargo:rerun-if-env-changed=LUNA_FLAVOR");
    if std::env::var("LUNA_FLAVOR").is_ok_and(|f| f.trim() == "test") {
        println!("cargo:rustc-cfg=luna_test");
    }
}
