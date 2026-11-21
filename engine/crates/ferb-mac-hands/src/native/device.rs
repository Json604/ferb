//! The Mac's state for `device.facts` (T-1130), public API only, each read on its own so one that fails leaves only
//! its fact out: the computer name and network (SystemConfiguration), the battery (IOKit power sources), Low Power
//! Mode, locale and languages (Foundation), the appearance (the global defaults' AppleInterfaceStyle), the displays
//! and whether the screen is locked (CoreGraphics; the session dictionary's `CGSSessionScreenIsLocked`), the default
//! output device and whether it plays (CoreAudio), the model, macOS version, boot and wake times (sysctl) and the
//! home volume's free space (statfs). Focus / Do Not Disturb has no public reading on macOS and is not read. The
//! Wi-Fi network's name needs the Location permission in the reading process: the daemon doesn't hold it (FERB.app
//! does), so it is not read here.

use std::ffi::{c_void, CStr, CString};
use std::ptr::NonNull;

use objc2_core_foundation::{CFArray, CFDictionary, CFRetained, CFString, CFType};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayIsBuiltin, CGGetActiveDisplayList,
    CGMainDisplayID, CGSessionCopyCurrentDictionary,
};
use objc2_foundation::{NSLocale, NSProcessInfo, NSString, NSUserDefaults};

use super::cf::{boolean, dict_get, num, text};
use crate::device::{
    audio_kind, network_kind, percent, Battery, Display, Hardware, Network, Reading,
};

#[link(name = "SystemConfiguration", kind = "framework")]
extern "C" {
    fn SCDynamicStoreCopyComputerName(store: *const c_void, encoding: *mut u32) -> *const c_void;
    fn SCDynamicStoreCreate(
        allocator: *const c_void,
        name: *const c_void,
        callout: *const c_void,
        context: *const c_void,
    ) -> *const c_void;
    fn SCDynamicStoreCopyValue(store: *const c_void, key: *const c_void) -> *const c_void;
    fn SCNetworkInterfaceCopyAll() -> *const c_void;
    fn SCNetworkInterfaceGetBSDName(interface: *const c_void) -> *const c_void;
    fn SCNetworkInterfaceGetInterfaceType(interface: *const c_void) -> *const c_void;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> *const c_void;
    fn IOPSCopyPowerSourcesList(blob: *const c_void) -> *const c_void;
    fn IOPSGetPowerSourceDescription(blob: *const c_void, ps: *const c_void) -> *const c_void;
}

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

/// A Create-rule CF object, owned (None for NULL).
fn owned<T: objc2_core_foundation::Type>(ptr: *const c_void) -> Option<CFRetained<T>> {
    // SAFETY: the callers pass a Create/Copy-rule result of the type named, or NULL.
    NonNull::new(ptr as *mut T).map(|p| unsafe { CFRetained::from_raw(p) })
}

/// A Get-rule CF object, retained for our use (None for NULL).
fn borrowed<T: objc2_core_foundation::Type>(ptr: *const c_void) -> Option<CFRetained<T>> {
    // SAFETY: the callers pass a Get-rule result of the type named, or NULL; retaining it keeps it alive.
    NonNull::new(ptr as *mut T).map(|p| unsafe { CFRetained::retain(p) })
}

fn key(s: &'static str) -> CFRetained<CFString> {
    CFString::from_static_str(s)
}

/// Everything readable now (each fact on its own).
pub fn read(home: &std::path::Path) -> Reading {
    super::drained(|| {
        let (disk_free_bytes, disk_total_bytes) = disk(home);
        let (audio_output, audio_playing, audio_kind) = audio();
        Reading {
            name: computer_name(),
            model: sysctl_text("hw.model"),
            macos: sysctl_text("kern.osproductversion"),
            battery: battery(),
            low_power_mode: Some(NSProcessInfo::processInfo().isLowPowerModeEnabled()),
            network: network(),
            screen_locked: screen_locked(),
            displays: displays(),
            disk_free_bytes,
            disk_total_bytes,
            booted_at: sysctl_time("kern.boottime"),
            woke_at: sysctl_time("kern.waketime"),
            audio_output,
            audio_playing,
            audio_kind,
            hardware: hardware(),
            appearance: Some(appearance()),
            locale: Some(NSLocale::currentLocale().localeIdentifier().to_string()),
            languages: NSLocale::preferredLanguages()
                .iter()
                .map(|l| l.to_string())
                .collect(),
        }
    })
}

/// The Mac's computer name (System Settings → General → About → Name).
pub fn computer_name() -> Option<String> {
    // SAFETY: a NULL store and encoding are documented as allowed; the result follows the Copy rule.
    let name: Option<CFRetained<CFString>> =
        owned(unsafe { SCDynamicStoreCopyComputerName(std::ptr::null(), std::ptr::null_mut()) });
    name.map(|n| n.to_string()).filter(|n| !n.trim().is_empty())
}

fn sysctl_text(name: &str) -> Option<String> {
    let c = CString::new(name).ok()?;
    let mut size: libc::size_t = 0;
    // SAFETY: asks only for the size of a named value.
    if unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || size == 0
    {
        return None;
    }
    let mut buf = vec![0u8; size];
    // SAFETY: the buffer is `size` bytes, as the call was told.
    if unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    let text = CStr::from_bytes_until_nul(&buf)
        .ok()?
        .to_string_lossy()
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

fn sysctl_time(name: &str) -> Option<f64> {
    let c = CString::new(name).ok()?;
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    let mut size = std::mem::size_of::<libc::timeval>();
    // SAFETY: a timeval-sized buffer for a timeval value.
    let rc = unsafe {
        libc::sysctlbyname(
            c.as_ptr(),
            (&mut tv as *mut libc::timeval).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && tv.tv_sec > 0).then(|| tv.tv_sec as f64 + f64::from(tv.tv_usec) / 1e6)
}

fn battery() -> Option<Option<Battery>> {
    // SAFETY: Copy-rule results, released by CFRetained; the description is Get-rule (owned by the blob).
    let blob: CFRetained<CFType> = owned(unsafe { IOPSCopyPowerSourcesInfo() })?;
    let list: CFRetained<CFArray> = owned(unsafe {
        IOPSCopyPowerSourcesList(CFRetained::as_ptr(&blob).as_ptr() as *const c_void)
    })?;
    // SAFETY: the list holds CF objects.
    let list: &CFArray<CFType> = unsafe { list.cast_unchecked() };
    for ps in list.iter() {
        let desc: Option<CFRetained<CFDictionary>> = borrowed(unsafe {
            IOPSGetPowerSourceDescription(
                CFRetained::as_ptr(&blob).as_ptr() as *const c_void,
                CFRetained::as_ptr(&ps).as_ptr() as *const c_void,
            )
        });
        let Some(desc) = desc else { continue };
        if text(dict_get(&desc, &key("Type"))).as_deref() != Some("InternalBattery") {
            continue;
        }
        let current = num(dict_get(&desc, &key("Current Capacity")))?;
        let max = num(dict_get(&desc, &key("Max Capacity"))).unwrap_or(100.0);
        return Some(Some(Battery {
            percent: percent(current, max),
            charging: boolean(dict_get(&desc, &key("Is Charging"))).unwrap_or(false),
            power_source: text(dict_get(&desc, &key("Power Source State"))).unwrap_or_default(),
        }));
    }
    Some(None)
}

fn network() -> Option<Network> {
    let name = key("ferb-device-facts");
    // SAFETY: a session with no callout; Create-rule result.
    let store: CFRetained<CFType> = owned(unsafe {
        SCDynamicStoreCreate(
            std::ptr::null(),
            CFRetained::as_ptr(&name).as_ptr() as *const c_void,
            std::ptr::null(),
            std::ptr::null(),
        )
    })?;
    let global = key("State:/Network/Global/IPv4");
    // SAFETY: Copy-rule result: the global IPv4 state, a dictionary, or NULL when there is no route.
    let state: Option<CFRetained<CFDictionary>> = owned(unsafe {
        SCDynamicStoreCopyValue(
            CFRetained::as_ptr(&store).as_ptr() as *const c_void,
            CFRetained::as_ptr(&global).as_ptr() as *const c_void,
        )
    });
    let Some(primary) = state.and_then(|s| text(dict_get(&s, &key("PrimaryInterface")))) else {
        return Some(Network {
            kind: "offline".into(),
            interface: String::new(),
        });
    };
    // SAFETY: Copy-rule array of SCNetworkInterfaces; their names and types are Get-rule CFStrings.
    let all: Option<CFRetained<CFArray>> = owned(unsafe { SCNetworkInterfaceCopyAll() });
    let mut kind = "other";
    if let Some(all) = all {
        // SAFETY: the array holds CF objects.
        let all: &CFArray<CFType> = unsafe { all.cast_unchecked() };
        for iface in all.iter() {
            let p = CFRetained::as_ptr(&iface).as_ptr() as *const c_void;
            let bsd: Option<CFRetained<CFString>> =
                borrowed(unsafe { SCNetworkInterfaceGetBSDName(p) });
            if bsd.map(|b| b.to_string()).as_deref() == Some(primary.as_str()) {
                let t: Option<CFRetained<CFString>> =
                    borrowed(unsafe { SCNetworkInterfaceGetInterfaceType(p) });
                kind = network_kind(&t.map(|t| t.to_string()).unwrap_or_default());
                break;
            }
        }
    }
    Some(Network {
        kind: kind.into(),
        interface: primary,
    })
}

fn screen_locked() -> Option<bool> {
    let session = CGSessionCopyCurrentDictionary()?;
    Some(boolean(dict_get(&session, &key("CGSSessionScreenIsLocked"))).unwrap_or(false))
}

fn displays() -> Vec<Display> {
    let mut ids: [CGDirectDisplayID; 16] = [0; 16];
    let mut count: u32 = 0;
    // SAFETY: room for 16 ids, as the call is told.
    let err = unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut count) };
    if err.0 != 0 {
        return Vec::new();
    }
    let main = CGMainDisplayID();
    ids[..count.min(16) as usize]
        .iter()
        .map(|id| {
            let b = CGDisplayBounds(*id);
            Display {
                width: b.size.width.round() as i64,
                height: b.size.height.round() as i64,
                main: *id == main,
                built_in: CGDisplayIsBuiltin(*id),
            }
        })
        .collect()
}

fn disk(home: &std::path::Path) -> (Option<u64>, Option<u64>) {
    let Ok(c) = CString::new(home.as_os_str().as_encoded_bytes()) else {
        return (None, None);
    };
    // SAFETY: statfs fills the struct for an existing path.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return (None, None);
    }
    let block = u64::from(st.f_bsize);
    (
        Some(st.f_bavail.saturating_mul(block)),
        Some(st.f_blocks.saturating_mul(block)),
    )
}

fn audio_u32(object: u32, selector: &[u8; 4]) -> Option<u32> {
    audio_u32_in(object, selector, b"glob")
}

fn audio_u32_in(object: u32, selector: &[u8; 4], scope: &[u8; 4]) -> Option<u32> {
    let addr = PropertyAddress {
        selector: fourcc(selector),
        scope: fourcc(scope),
        element: 0,
    };
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: a u32-sized buffer for a u32 property.
    let rc = unsafe {
        AudioObjectGetPropertyData(
            object,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            (&mut value as *mut u32).cast(),
        )
    };
    (rc == 0).then_some(value)
}

fn audio() -> (Option<String>, Option<bool>, Option<String>) {
    const SYSTEM: u32 = 1; // kAudioObjectSystemObject
    let Some(device) = audio_u32(SYSTEM, b"dOut").filter(|d| *d != 0) else {
        return (None, None, None);
    };
    // kAudioDevicePropertyTransportType, and the built-in device's output data source (speakers or headphones)
    let kind = audio_u32(device, b"tran")
        .map(|t| audio_kind(t, audio_u32_in(device, b"ssrc", b"outp")).to_string());
    let addr = PropertyAddress {
        selector: fourcc(b"lnam"),
        scope: fourcc(b"glob"),
        element: 0,
    };
    let mut name: *const c_void = std::ptr::null();
    let mut size = std::mem::size_of::<*const c_void>() as u32;
    // SAFETY: kAudioObjectPropertyName gives a Create-rule CFString into a pointer-sized buffer.
    let rc = unsafe {
        AudioObjectGetPropertyData(
            device,
            &addr,
            0,
            std::ptr::null(),
            &mut size,
            (&mut name as *mut *const c_void).cast(),
        )
    };
    let shown = if rc == 0 {
        owned::<CFString>(name).map(|n| n.to_string())
    } else {
        None
    };
    // kAudioDevicePropertyDeviceIsRunningSomewhere: some process plays through the device now
    (shown, audio_u32(device, b"gone").map(|v| v != 0), kind)
}

/// The Mac's hardware: the device tree's product-name (Apple silicon), the chip and the memory. Never the serial.
pub fn hardware() -> Hardware {
    let mut size = std::mem::size_of::<u64>();
    let mut memory: u64 = 0;
    let name = CString::new("hw.memsize").ok();
    // SAFETY: a u64-sized buffer for hw.memsize.
    let ok = name.is_some_and(|n| unsafe {
        libc::sysctlbyname(
            n.as_ptr(),
            (&mut memory as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0);
    Hardware {
        name: product_name(),
        chip: sysctl_text("machdep.cpu.brand_string"),
        memory_bytes: (ok && memory > 0).then_some(memory),
    }
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IORegistryEntryFromPath(main: u32, path: *const std::ffi::c_char) -> u32;
    fn IORegistryEntryCreateCFProperty(
        entry: u32,
        key: *const c_void,
        alloc: *const c_void,
        opts: u32,
    ) -> *const c_void;
    fn IOObjectRelease(obj: u32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFDataGetLength(d: *const c_void) -> isize;
    fn CFDataGetBytePtr(d: *const c_void) -> *const u8;
    fn CFGetTypeID(cf: *const c_void) -> usize;
    fn CFDataGetTypeID() -> usize;
}

/// "MacBook Air (13-inch, M4, 2025)": IODeviceTree:/product's product-name (bytes ending in NUL).
fn product_name() -> Option<String> {
    let path = CString::new("IODeviceTree:/product").ok()?;
    // SAFETY: a NUL-terminated registry path; the main port 0 is the default.
    let entry = unsafe { IORegistryEntryFromPath(0, path.as_ptr()) };
    if entry == 0 {
        return None;
    }
    let k = key("product-name");
    // SAFETY: a registry entry this code holds; the property follows the Create rule (owned below).
    let prop: Option<CFRetained<CFType>> = owned(unsafe {
        IORegistryEntryCreateCFProperty(
            entry,
            CFRetained::as_ptr(&k).as_ptr() as *const c_void,
            std::ptr::null(),
            0,
        )
    });
    // SAFETY: the entry from IORegistryEntryFromPath, released once.
    unsafe { IOObjectRelease(entry) };
    let prop = prop?;
    let p = CFRetained::as_ptr(&prop).as_ptr() as *const c_void;
    // SAFETY: a type-id read, then a CFData's length and bytes, valid while `prop` lives.
    let bytes = unsafe {
        if CFGetTypeID(p) != CFDataGetTypeID() {
            return None;
        }
        std::slice::from_raw_parts(CFDataGetBytePtr(p), CFDataGetLength(p).max(0) as usize)
    };
    let text = String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

fn appearance() -> String {
    let style = NSUserDefaults::standardUserDefaults()
        .stringForKey(&NSString::from_str("AppleInterfaceStyle"));
    if style.is_some_and(|s| s.to_string().eq_ignore_ascii_case("dark")) {
        "dark".into()
    } else {
        "light".into()
    }
}

#[cfg(test)]
mod tests {
    /// By hand only (TEST-508: it reads this Mac): every public reading answers on a real Mac.
    #[test]
    #[ignore = "by hand: reads this Mac's state"]
    fn reads_this_mac() {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let r = super::read(&home);
        println!("{r:#?}");
        assert!(r.model.is_some() && r.macos.is_some() && !r.displays.is_empty());
    }
}
