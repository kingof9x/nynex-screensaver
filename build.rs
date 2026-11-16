// Windows only draws layered *child* windows (how the wallpaper sits on the 24H2+ desktop)
// for programs whose manifest says they support Windows 8 or later, so embed one.
fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest::embed_manifest(embed_manifest::new_manifest("nynex")).expect("could not embed the Windows manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
