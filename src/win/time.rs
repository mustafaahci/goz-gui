//! Times: the system's clock, and times converted between their forms
//! (FILETIME, SYSTEMTIME) and between UTC and the current time zone.

use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows_sys::Win32::System::Time::{
    FileTimeToSystemTime, SystemTimeToFileTime, SystemTimeToTzSpecificLocalTime,
    TzSpecificLocalTimeToSystemTime,
};

/// The system's precise time now (GetSystemTimePreciseAsFileTime): 100 ns
/// since 1601, as a FILETIME counts.
pub fn precise_time() -> u64 {
    let mut ft = FILETIME::default();
    // SAFETY: an out-struct.
    unsafe { GetSystemTimePreciseAsFileTime(&mut ft) };
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// A local time (the current time zone's, by its rules for that date) as
/// UTC; `None` if Windows will not convert it.
pub fn local_to_utc(local: &SYSTEMTIME) -> Option<SYSTEMTIME> {
    let mut utc = SYSTEMTIME::default();
    // SAFETY: an in-struct and an out-struct; a null time zone is the
    // current one.
    (unsafe { TzSpecificLocalTimeToSystemTime(core::ptr::null(), local, &mut utc) } != 0)
        .then_some(utc)
}

/// A UTC time in the current time zone; `None` if Windows will not convert
/// it.
pub fn utc_to_local(utc: &SYSTEMTIME) -> Option<SYSTEMTIME> {
    let mut local = SYSTEMTIME::default();
    // SAFETY: an in-struct and an out-struct; a null time zone is the
    // current one.
    (unsafe { SystemTimeToTzSpecificLocalTime(core::ptr::null(), utc, &mut local) } != 0)
        .then_some(local)
}

/// A SYSTEMTIME as a FILETIME (in the same zone); `None` if it is out of
/// range.
pub fn system_time_to_file_time(st: &SYSTEMTIME) -> Option<FILETIME> {
    let mut ft = FILETIME::default();
    // SAFETY: an in-struct and an out-struct.
    (unsafe { SystemTimeToFileTime(st, &mut ft) } != 0).then_some(ft)
}

/// A FILETIME as a SYSTEMTIME (in the same zone); `None` if it is out of
/// range.
pub fn file_time_to_system_time(ft: &FILETIME) -> Option<SYSTEMTIME> {
    let mut st = SYSTEMTIME::default();
    // SAFETY: an in-struct and an out-struct.
    (unsafe { FileTimeToSystemTime(ft, &mut st) } != 0).then_some(st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_goes_there_and_back() {
        let st = SYSTEMTIME {
            wYear: 2024,
            wMonth: 2,
            wDay: 29,
            wHour: 13,
            wMinute: 5,
            ..Default::default()
        };
        let ft = system_time_to_file_time(&st).unwrap();
        let back = file_time_to_system_time(&ft).unwrap();
        assert_eq!((back.wYear, back.wMonth, back.wDay), (2024, 2, 29));
        let local = utc_to_local(&st).unwrap();
        let utc = local_to_utc(&local).unwrap();
        assert_eq!((utc.wHour, utc.wMinute), (13, 5));
        assert!(precise_time() > ft.dwLowDateTime as u64);
    }
}
