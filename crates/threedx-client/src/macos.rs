//! The driver's client API, called through function pointers from `dlopen`.
//!
//! No safe API exists for this, hence the `unsafe_code` allowance for the whole module. The
//! unsafe parts are small: the C declarations, the lookups and the two callbacks, which read the
//! message the driver hands them.

#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU16, Ordering};

use crate::{Error, Event};

const FRAMEWORK: &str = "/Library/Frameworks/3DconnexionClient.framework/3DconnexionClient";
const RTLD_NOW: c_int = 2;

/// `'****'`: any application signature.
const WILDCARD_SIGNATURE: u32 = 0x2A2A_2A2A;
/// The driver gives this client the cap, instead of sharing it with other clients.
const MODE_TAKE_OVER: u16 = 1;
/// Every axis and every button.
const MASK_ALL: u32 = 0x3FFF;
/// `'3dSR'`: the message that carries a device state.
const MSG_DEVICE_STATE: u32 = 0x3364_5352;
const CMD_BUTTONS: u16 = 2;
const CMD_AXES: u16 = 3;

/// The device state the driver passes, read field by field from these byte offsets (the struct is
/// packed to two bytes) so that no reference to driver memory is ever formed.
const STATE_BYTES: usize = 48;
const OFFSET_CLIENT: usize = 2;
const OFFSET_COMMAND: usize = 4;
const OFFSET_AXES: usize = 30;
const OFFSET_BUTTONS: usize = 44;

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopDefaultMode: *const c_void;
    fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
}

type MessageHandler = extern "C" fn(u32, u32, *mut c_void);
type DeviceHandler = extern "C" fn(u32);
type InstallHandlers = unsafe extern "C" fn(Option<MessageHandler>, Option<DeviceHandler>, Option<DeviceHandler>) -> i16;
type RegisterClient = unsafe extern "C" fn(u32, *const u8, u16, u32) -> u16;
type SetMask = unsafe extern "C" fn(u16, u32);

type Sink = Box<dyn Fn(Event) + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();
/// The id the driver gave this client: it broadcasts every message to every client.
static CLIENT: AtomicU16 = AtomicU16::new(0);

fn symbol(handle: *mut c_void, name: &'static str) -> Result<*mut c_void, Error> {
    let c_name = CString::new(name).map_err(|_| Error::MissingSymbol(name))?;
    // SAFETY: `handle` came from a successful dlopen and `c_name` is a NUL-terminated string.
    let found = unsafe { dlsym(handle, c_name.as_ptr()) };
    if found.is_null() { Err(Error::MissingSymbol(name)) } else { Ok(found) }
}

fn emit(event: Event) {
    if let Some(sink) = SINK.get() {
        // A panic must not unwind into the driver's C code.
        let _ = catch_unwind(AssertUnwindSafe(|| sink(event)));
    }
}

extern "C" fn on_added(_connection: u32) {
    emit(Event::DeviceAdded);
}

extern "C" fn on_removed(_connection: u32) {
    emit(Event::DeviceRemoved);
}

extern "C" fn on_message(_connection: u32, kind: u32, argument: *mut c_void) {
    if kind != MSG_DEVICE_STATE || argument.is_null() {
        return;
    }
    // SAFETY: for this message type the driver passes a device state, which is `STATE_BYTES` long
    // and valid for the duration of the call.
    let state = unsafe { std::slice::from_raw_parts(argument.cast::<u8>().cast_const(), STATE_BYTES) };
    if let Some(event) = decode(state, CLIENT.load(Ordering::Relaxed)) {
        emit(event);
    }
}

fn u16_at(state: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(state.get(offset..offset + 2)?.try_into().ok()?))
}

fn i16_at(state: &[u8], offset: usize) -> Option<i16> {
    Some(i16::from_ne_bytes(state.get(offset..offset + 2)?.try_into().ok()?))
}

/// The event a device state stands for, if it is meant for `client`.
fn decode(state: &[u8], client: u16) -> Option<Event> {
    if u16_at(state, OFFSET_CLIENT)? != client {
        return None;
    }
    match u16_at(state, OFFSET_COMMAND)? {
        CMD_AXES => {
            let axis = |i: usize| i16_at(state, OFFSET_AXES + 2 * i);
            Some(Event::Axes { translation: [axis(0)?, axis(1)?, axis(2)?], rotation: [axis(3)?, axis(4)?, axis(5)?] })
        }
        CMD_BUTTONS => Some(Event::Buttons(u32::from_ne_bytes(state.get(OFFSET_BUTTONS..OFFSET_BUTTONS + 4)?.try_into().ok()?))),
        _ => None,
    }
}

/// Loads the framework, installs the handlers and registers the client. Runs on the client thread,
/// whose run loop the driver then sends its messages to.
fn connect(name: &str) -> Result<(), Error> {
    let path = CString::new(FRAMEWORK).map_err(|_| Error::NotInstalled)?;
    // SAFETY: `path` is a NUL-terminated string.
    let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW) };
    if handle.is_null() {
        return Err(Error::NotInstalled);
    }
    let install = symbol(handle, "InstallConnexionHandlers")?;
    let register = symbol(handle, "RegisterConnexionClient")?;
    // SAFETY: the framework declares these functions with exactly these signatures.
    let (install, register) =
        unsafe { (std::mem::transmute::<*mut c_void, InstallHandlers>(install), std::mem::transmute::<*mut c_void, RegisterClient>(register)) };
    // SAFETY: the handlers are `extern "C"` functions that live for the whole process.
    let code = unsafe { install(Some(on_message), Some(on_added), Some(on_removed)) };
    if code != 0 {
        return Err(Error::Handlers(code));
    }
    // The name is a Pascal string: a length byte, then the characters.
    let mut pascal = vec![0_u8];
    pascal.extend(name.bytes().take(255));
    pascal[0] = u8::try_from(pascal.len() - 1).unwrap_or(0);
    pascal.push(0);
    // SAFETY: `pascal` outlives the call, which copies it.
    let id = unsafe { register(WILDCARD_SIGNATURE, pascal.as_ptr(), MODE_TAKE_OVER, MASK_ALL) };
    if id == 0 {
        return Err(Error::Register);
    }
    CLIENT.store(id, Ordering::Relaxed);
    // Newer drivers want the masks again once the client has an id. Both are optional.
    for (name, mask) in [("SetConnexionClientMask", MASK_ALL), ("SetConnexionClientButtonMask", u32::MAX)] {
        if let Ok(set) = symbol(handle, name) {
            // SAFETY: the framework declares these functions with this signature.
            let set = unsafe { std::mem::transmute::<*mut c_void, SetMask>(set) };
            // SAFETY: `id` is the id the driver gave this client.
            unsafe { set(id, mask) };
        }
    }
    Ok(())
}

pub fn start(name: &str, sink: Sink) -> Result<(), Error> {
    SINK.set(sink).map_err(|_| Error::AlreadyStarted)?;
    let name = name.to_owned();
    let (ready, started) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("3dx-client".into())
        .spawn(move || {
            let result = connect(&name);
            let connected = result.is_ok();
            let _ = ready.send(result);
            if !connected {
                return;
            }
            // The driver's messages arrive on this thread's run loop. Waking up now and then keeps
            // the loop alive in case it had nothing to wait for yet.
            loop {
                // SAFETY: the mode is CoreFoundation's own constant.
                unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0, 0) };
            }
        })
        .map_err(|error| Error::Thread(error.to_string()))?;
    started.recv().map_err(|error| Error::Thread(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device state as the driver sent it for a light tilt forward (captured from a SpaceMouse
    /// Wireless): command 3 (axes) with the fourth axis, tilt forward/back, at -3.
    fn tilt_state(client: u16) -> [u8; STATE_BYTES] {
        let mut state = [0_u8; STATE_BYTES];
        state[OFFSET_CLIENT..OFFSET_CLIENT + 2].copy_from_slice(&client.to_ne_bytes());
        state[OFFSET_COMMAND..OFFSET_COMMAND + 2].copy_from_slice(&CMD_AXES.to_ne_bytes());
        state[OFFSET_AXES + 6..OFFSET_AXES + 8].copy_from_slice(&(-3_i16).to_ne_bytes());
        state
    }

    #[test]
    fn an_axes_state_for_this_client_becomes_axes() {
        assert_eq!(decode(&tilt_state(4106), 4106), Some(Event::Axes { translation: [0; 3], rotation: [-3, 0, 0] }));
    }

    #[test]
    fn a_state_for_another_client_is_ignored() {
        assert_eq!(decode(&tilt_state(4106), 4107), None);
    }

    #[test]
    fn a_button_state_carries_the_mask() {
        let mut state = tilt_state(7);
        state[OFFSET_COMMAND..OFFSET_COMMAND + 2].copy_from_slice(&CMD_BUTTONS.to_ne_bytes());
        state[OFFSET_BUTTONS..OFFSET_BUTTONS + 4].copy_from_slice(&5_u32.to_ne_bytes());
        assert_eq!(decode(&state, 7), Some(Event::Buttons(5)));
    }

    #[test]
    fn a_short_message_is_ignored() {
        assert_eq!(decode(&[0; 10], 0), None);
    }
}
