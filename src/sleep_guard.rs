//! Prevent the system from going to idle sleep while jcode is doing work
//! (e.g. streaming a model response).
//!
//! On macOS this holds an IOKit power assertion (`PreventUserIdleSystemSleep`),
//! which is the same mechanism `caffeinate -i` uses. It keeps the machine awake
//! through idle-sleep timers but intentionally does not block lid-close sleep
//! or display sleep. On other platforms this is currently a no-op.
//!
//! Set `JCODE_DISABLE_SLEEP_GUARD=1` to opt out.

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CString, c_char, c_void};

    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFAllocatorRef = *const c_void;
    type IOPMAssertionID = u32;
    type IOReturn = i32;

    const ASSERTION_TYPE_PREVENT_USER_IDLE_SYSTEM_SLEEP: &str = "PreventUserIdleSystemSleep";
    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;
    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
    const K_IO_RETURN_SUCCESS: IOReturn = 0;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: CFAllocatorRef,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            assertion_level: u32,
            assertion_name: CFStringRef,
            assertion_id: *mut IOPMAssertionID,
        ) -> IOReturn;
        fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> IOReturn;
    }

    pub(super) fn create(name: &str) -> Option<u32> {
        let assertion_type = CString::new(ASSERTION_TYPE_PREVENT_USER_IDLE_SYSTEM_SLEEP).ok()?;
        let assertion_name = CString::new(name).ok()?;
        unsafe {
            let type_ref = CFStringCreateWithCString(
                std::ptr::null(),
                assertion_type.as_ptr(),
                K_CF_STRING_ENCODING_UTF8,
            );
            if type_ref.is_null() {
                return None;
            }
            let name_ref = CFStringCreateWithCString(
                std::ptr::null(),
                assertion_name.as_ptr(),
                K_CF_STRING_ENCODING_UTF8,
            );
            if name_ref.is_null() {
                CFRelease(type_ref);
                return None;
            }
            let mut assertion_id: IOPMAssertionID = 0;
            let status = IOPMAssertionCreateWithName(
                type_ref,
                K_IOPM_ASSERTION_LEVEL_ON,
                name_ref,
                &mut assertion_id,
            );
            CFRelease(type_ref);
            CFRelease(name_ref);
            (status == K_IO_RETURN_SUCCESS).then_some(assertion_id)
        }
    }

    pub(super) fn release(assertion_id: u32) {
        unsafe {
            IOPMAssertionRelease(assertion_id);
        }
    }
}

fn disabled_by_env() -> bool {
    std::env::var("JCODE_DISABLE_SLEEP_GUARD")
        .map(|value| {
            let value = value.trim();
            !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

/// RAII guard that keeps the system awake (no idle sleep) while it is alive.
///
/// Dropping the guard releases the underlying power assertion immediately.
pub struct SleepGuard {
    #[cfg(target_os = "macos")]
    assertion_id: u32,
}

impl SleepGuard {
    /// Acquire a sleep-prevention assertion. Returns `None` on unsupported
    /// platforms, when disabled via `JCODE_DISABLE_SLEEP_GUARD`, or on failure.
    ///
    /// `reason` is visible to the user in `pmset -g assertions`.
    pub fn acquire(reason: &str) -> Option<Self> {
        if disabled_by_env() {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            let assertion_id = imp::create(reason)?;
            crate::logging::debug(&format!(
                "sleep_guard: acquired power assertion {assertion_id} ({reason})"
            ));
            Some(Self { assertion_id })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reason;
            None
        }
    }
}

impl Drop for SleepGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            imp::release(self.assertion_id);
            crate::logging::debug(&format!(
                "sleep_guard: released power assertion {}",
                self.assertion_id
            ));
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::SleepGuard;
    use std::process::Command;

    fn pmset_assertions() -> String {
        let output = Command::new("/usr/bin/pmset")
            .args(["-g", "assertions"])
            .output()
            .expect("pmset -g assertions should run");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[test]
    fn assertion_is_visible_in_pmset_and_released_on_drop() {
        let name = format!("jcode-sleep-guard-test-{}", std::process::id());

        let guard = SleepGuard::acquire(&name).expect("should acquire power assertion");
        let while_held = pmset_assertions();
        assert!(
            while_held.contains(&name),
            "expected assertion named {name} in pmset output while guard held:\n{while_held}"
        );
        assert!(
            while_held.contains("PreventUserIdleSystemSleep"),
            "expected PreventUserIdleSystemSleep in pmset output:\n{while_held}"
        );

        drop(guard);
        let after_drop = pmset_assertions();
        assert!(
            !after_drop.contains(&name),
            "expected assertion named {name} to be gone after drop:\n{after_drop}"
        );
    }
}
