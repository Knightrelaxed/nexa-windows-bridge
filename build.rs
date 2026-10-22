fn main() {
    slint_build::compile("ui/main_window.slint").unwrap();

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "N.E.X.A Windows Bridge");
        res.set("FileDescription", "N.E.X.A Native Windows Desktop Companion");
        res.set("LegalCopyright", "Copyright (C) 2026 Faqih / N.E.X.A Team");
        if let Err(e) = res.compile() {
            eprintln!("Warning: Failed to compile Windows resource: {}", e);
        }
    }
}
