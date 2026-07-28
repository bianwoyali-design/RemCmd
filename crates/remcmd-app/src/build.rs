#[cfg(target_os = "windows")]
fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons/remcmd.ico");

    let mut resources = winres::WindowsResource::new();
    resources.set_icon("../../assets/icons/remcmd.ico");
    resources.compile().expect("failed to embed Windows icon");
}

#[cfg(not(target_os = "windows"))]
fn main() {}
