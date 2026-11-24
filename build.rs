// Icône, nom et marqueur CLI de l'application dans l'exe : visibles dans l'Explorateur Windows,
// la barre des tâches et le Gestionnaire des tâches (qui affiche « Lanceur »).
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("FileDescription", "Lanceur")
            .set("ProductName", "Lanceur")
            .set("InternalName", "lanceur")
            .set("OriginalFilename", "Lanceur.exe")
            .set("CompanyName", "Lanceur")
            .set("LegalCopyright", "")
            // Marqueur pour les autres applis (comme VigieCli) : options de ligne de commande prises en charge.
            .set("LanceurCli", "resume");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icône non intégrée : {e}");
        }
    }
}
