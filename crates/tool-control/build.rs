fn main() {
    println!("cargo:rerun-if-changed=../../assets/windows/app.ico");
    if cfg!(target_os = "windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("../../assets/windows/app.ico");
        resource.set("ProductName", "Cocos Build LAN");
        resource.set("FileDescription", "Cocos Build LAN Control");
        resource
            .compile()
            .expect("compile Windows control resources");
    }
}
