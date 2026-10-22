fn main() {
    slint_build::compile("ui/main_window.slint").unwrap();

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Nexa Bridge");
        res.set("FileDescription", "Nexa Bridge");
        res.set("LegalCopyright", "Copyright (C) 2026 Faqih / Nexa Team");
        if let Err(e) = res.compile() {
            eprintln!("Warning: Failed to compile Windows resource: {}", e);
        }
    }
}
