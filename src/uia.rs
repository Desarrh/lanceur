//! Texte sélectionné dans l'app au premier plan, via UI Automation (sans presse-papiers).

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationTextPattern, UIA_TextPatternId,
};
use windows::core::Interface;

/// Texte sélectionné dans l'élément qui a le focus clavier (rogné), None si vide ou sans TextPattern.
/// Initialise COM (MTA) sur le thread appelant le temps de l'appel.
pub fn selected_text() -> Option<String> {
    let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let s = read(); // les objets COM sont libérés avant CoUninitialize
    if init {
        unsafe { CoUninitialize() };
    }
    let s = s?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn read() -> Option<String> {
    unsafe {
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        // Délais courts pour ne pas rester bloqué sur une app qui ne répond pas (Windows 8+).
        if let Ok(u2) = uia.cast::<IUIAutomation2>() {
            let _ = u2.SetConnectionTimeout(100);
            let _ = u2.SetTransactionTimeout(100);
        }
        let tp: IUIAutomationTextPattern =
            uia.GetFocusedElement().ok()?.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
        let ranges = tp.GetSelection().ok()?;
        if ranges.Length().ok()? < 1 {
            return None;
        }
        Some(ranges.GetElement(0).ok()?.GetText(10000).ok()?.to_string())
    }
}

#[cfg(test)]
mod tests {
    // N'affiche jamais le texte lu : il peut être privé.
    #[test]
    fn rend_la_main_vite() {
        let t = std::time::Instant::now();
        let r = std::thread::spawn(super::selected_text).join();
        assert!(r.is_ok(), "selected_text() a paniqué");
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
    }
}
