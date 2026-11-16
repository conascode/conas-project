fn main() {
    println!("cargo:rerun-if-changed=gaeunmemo2.rc");
    println!("cargo:rerun-if-changed=app.manifest");
    // PerMonitorV2 DPI, comctl32 v6, UTF-8 code page
    embed_resource::compile("gaeunmemo2.rc", embed_resource::NONE).manifest_required().unwrap();
}
