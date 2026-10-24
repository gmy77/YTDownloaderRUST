// Incorpora icona e informazioni di versione nell'exe.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "YTDownloaderRUST");
        res.set("ProductName", "YTDownloaderRUST");
        res.compile().expect("compilazione delle risorse Windows non riuscita");
    }
}
