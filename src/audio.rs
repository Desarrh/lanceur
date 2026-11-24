//! Volume et sortie audio par défaut (Core Audio / MMDevice).

use std::ffi::c_void;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, ERole, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eCommunications,
    eConsole, eMultimedia, eRender,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, STGM_READ,
};
use windows::core::{GUID, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, Interface, PCWSTR, PWSTR};

pub struct Device {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// Initialise COM (MTA) sur le thread appelant puis exécute `f`, erreurs converties en texte.
/// Un appartement déjà initialisé (STA ou MTA) convient aussi.
fn run<T>(f: impl FnOnce() -> windows::core::Result<T>) -> Result<T, String> {
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        return Err(format!("CoInitializeEx : {hr}"));
    }
    f().map_err(|e| e.to_string())
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn endpoint() -> windows::core::Result<IAudioEndpointVolume> {
    unsafe { enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole)?.Activate(CLSCTX_ALL, None) }
}

/// Copie une chaîne allouée par COM puis la libère.
fn take(p: PWSTR) -> String {
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

fn id_of(d: &IMMDevice) -> windows::core::Result<String> {
    Ok(take(unsafe { d.GetId()? }))
}

fn name_of(d: &IMMDevice) -> windows::core::Result<String> {
    unsafe {
        let mut v = d.OpenPropertyStore(STGM_READ)?.GetValue(&PKEY_Device_FriendlyName)?;
        let s = PropVariantToStringAlloc(&v).map(take);
        let _ = PropVariantClear(&mut v);
        s
    }
}

/// Volume (0..1) et état muet de la sortie par défaut.
#[cfg_attr(not(test), allow(dead_code))] // lu par les tests (lecture seule)
pub fn volume() -> Result<(f32, bool), String> {
    run(|| unsafe {
        let e = endpoint()?;
        Ok((e.GetMasterVolumeLevelScalar()?, e.GetMute()?.as_bool()))
    })
}

/// Règle le volume (borné 0..1) ; démute si v > 0.
pub fn set_volume(v: f32) -> Result<(), String> {
    let v = v.clamp(0.0, 1.0);
    run(|| unsafe {
        let e = endpoint()?;
        e.SetMasterVolumeLevelScalar(v, std::ptr::null())?;
        if v > 0.0 {
            e.SetMute(false, std::ptr::null())?;
        }
        Ok(())
    })
}

/// Inverse le muet ; renvoie le nouvel état.
pub fn toggle_mute() -> Result<bool, String> {
    run(|| unsafe {
        let e = endpoint()?;
        let m = !e.GetMute()?.as_bool();
        e.SetMute(m, std::ptr::null())?;
        Ok(m)
    })
}

/// Périphériques de rendu actifs avec leur nom convivial.
pub fn outputs() -> Result<Vec<Device>, String> {
    run(|| unsafe {
        let en = enumerator()?;
        // Aucune sortie par défaut (aucun périphérique) : pas une erreur.
        let def = en.GetDefaultAudioEndpoint(eRender, eConsole).and_then(|d| id_of(&d)).ok();
        let col = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        (0..col.GetCount()?)
            .map(|i| {
                let d = col.Item(i)?;
                let id = id_of(&d)?;
                Ok(Device { name: name_of(&d)?, default: def.as_deref() == Some(id.as_str()), id })
            })
            .collect()
    })
}

// IPolicyConfig (non documentée). Ordre de la vtable après IUnknown :
// GetMixFormat, GetDeviceFormat, ResetDeviceFormat, SetDeviceFormat, GetProcessingPeriod,
// SetProcessingPeriod, GetShareMode, SetShareMode, GetPropertyValue, SetPropertyValue,
// SetDefaultEndpoint, SetEndpointVisibility.
// SetDefaultEndpoint est donc la 11e méthode après IUnknown, soit l'index 13 de la vtable complète.
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

#[repr(C)]
struct PolicyConfigVtbl {
    base: IUnknown_Vtbl,
    _avant: [usize; 10],
    set_default_endpoint: unsafe extern "system" fn(*mut c_void, PCWSTR, ERole) -> HRESULT,
}

/// Change la sortie par défaut pour les trois rôles (console, multimédia, communications).
pub fn set_default_output(id: &str) -> Result<(), String> {
    let id = HSTRING::from(id);
    run(|| unsafe {
        let unk: IUnknown = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
        let mut raw = std::ptr::null_mut();
        unk.query(&IID_POLICY_CONFIG, &mut raw).ok()?;
        let pc = IUnknown::from_raw(raw); // libéré à la sortie
        let vtbl = &**(pc.as_raw() as *const *const PolicyConfigVtbl);
        for role in [eConsole, eMultimedia, eCommunications] {
            (vtbl.set_default_endpoint)(pc.as_raw(), PCWSTR(id.as_ptr()), role).ok()?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    // Lecture seule : aucun test ne modifie le volume, le muet ni la sortie par défaut.

    #[test]
    fn outputs_au_plus_un_defaut() {
        let v = super::outputs().expect("outputs()");
        assert!(v.iter().filter(|d| d.default).count() <= 1);
    }

    #[test]
    fn volume_borne() {
        match super::volume() {
            Ok((v, _)) => assert!((0.0..=1.0).contains(&v)),
            // Sans périphérique audio, une erreur propre est acceptée.
            Err(e) => assert!(super::outputs().is_ok_and(|o| o.is_empty()), "volume() : {e}"),
        }
    }
}
