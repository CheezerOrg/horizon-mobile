//! The SRV question on iOS, where an app can't read `/etc/resolv.conf`: the system resolver
//! (`DNSServiceQueryRecord`, part of libSystem) asks the name servers of the network in use,
//! VPN and cellular ones included.
use std::{
    ffi::{CString, c_char, c_int, c_void},
    time::{Duration, Instant},
};

const TYPE_SRV: u16 = 33;
/// `kDNSServiceFlagsReturnIntermediates`: a missing record answers at once as
/// `kDNSServiceErr_NoSuchRecord` instead of when the wait runs out.
const RETURN_INTERMEDIATES: u32 = 0x1000;
const FLAG_ADD: u32 = 0x2;

type Callback = extern "C" fn(
    service: *mut c_void,
    flags: u32,
    interface: u32,
    error: i32,
    name: *const c_char,
    rr_type: u16,
    rr_class: u16,
    rd_len: u16,
    rdata: *const c_void,
    ttl: u32,
    context: *mut c_void,
);

unsafe extern "C" {
    fn DNSServiceQueryRecord(
        service: *mut *mut c_void,
        flags: u32,
        interface: u32,
        name: *const c_char,
        rr_type: u16,
        rr_class: u16,
        callback: Callback,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceRefSockFD(service: *mut c_void) -> c_int;
    fn DNSServiceProcessResult(service: *mut c_void) -> i32;
    fn DNSServiceRefDeallocate(service: *mut c_void);
}

/// What the callback heard: nothing yet, or the answer (the first SRV record, if any).
type Heard = Option<Option<(String, u16)>>;

extern "C" fn on_record(
    _: *mut c_void,
    flags: u32,
    _: u32,
    error: i32,
    _: *const c_char,
    rr_type: u16,
    _: u16,
    rd_len: u16,
    rdata: *const c_void,
    _: u32,
    context: *mut c_void,
) {
    // SAFETY: `context` is the `Heard` that `query_srv` keeps alive while it processes results.
    let heard = unsafe { &mut *context.cast::<Heard>() };
    if heard.is_some() {
        return;
    }
    if error != 0 {
        *heard = Some(None);
        return;
    }
    if rr_type != TYPE_SRV || flags & FLAG_ADD == 0 || rd_len < 7 {
        return;
    }
    // SAFETY: dns_sd hands `rd_len` bytes of record data for the length of the call.
    let data = unsafe { std::slice::from_raw_parts(rdata.cast::<u8>(), rd_len as usize) };
    // Priority, weight, port, then the target as uncompressed labels.
    let port = u16::from_be_bytes([data[4], data[5]]);
    *heard = Some(super::dns::read_name(data, 6).map(|(target, _)| (target, port)));
}

pub(super) fn query_srv(name: &str, timeout: Duration) -> Option<(String, u16)> {
    let name = CString::new(name).ok()?;
    let mut heard: Heard = None;
    let mut service = std::ptr::null_mut();
    // SAFETY: `service` and `heard` outlive the query, which is deallocated below.
    let error = unsafe {
        DNSServiceQueryRecord(
            &mut service,
            RETURN_INTERMEDIATES,
            0,
            name.as_ptr(),
            TYPE_SRV,
            1, // IN
            on_record,
            (&raw mut heard).cast(),
        )
    };
    if error != 0 {
        return None;
    }
    let deadline = Instant::now() + timeout;
    // SAFETY: `service` is a live query until `DNSServiceRefDeallocate`.
    let fd = unsafe { DNSServiceRefSockFD(service) };
    while heard.is_none() && fd >= 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut poll, 1, left.as_millis().max(1) as c_int) };
        // SAFETY: as above; the callback runs inside this call.
        if ready <= 0 || unsafe { DNSServiceProcessResult(service) } != 0 {
            break;
        }
    }
    // SAFETY: deallocated once, after the last use.
    unsafe { DNSServiceRefDeallocate(service) };
    heard.flatten()
}
