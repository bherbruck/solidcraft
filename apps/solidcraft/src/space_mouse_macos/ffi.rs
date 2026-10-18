//! The sole unsafe boundary for the installed macOS Navigation Library. ABI declarations are
//! independently expressed from the installed SDK's public C interface; no SDK source or binary
//! is vendored. Missing libraries/symbols are errors, not launch failures. Every pointer passed
//! into the driver has an owner retained until NlClose completes.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_long};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use libloading::Library;

use super::{Bounds, Matrix, State, Value};

// Keep callback state outside the driver: a late callback after Close can find no state rather
// than dereferencing a freed user-data pointer. Do not hold this lock across native calls.
static CALLBACK: Mutex<Option<Arc<Mutex<State>>>> = Mutex::new(None);
// make_result_code(ENOSYS), using macOS's ENOSYS=78 and the public Navlib facility=4.
const UNSUPPORTED: c_long = 0x8004_004e;

#[repr(C)]
#[derive(Clone, Copy)]
struct Text {
    pointer: *const c_char,
    length: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
union Data {
    boolean: u32,
    integer: c_long,
    number: f64,
    point: [f64; 3],
    plane: [f64; 4],
    bounds: Bounds,
    frustum: [f64; 6],
    matrix: Matrix,
    text: Text,
    node: *const Node,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeValue {
    kind: i32,
    data: Data,
}

type Get = extern "C" fn(u64, *const c_char, *mut NativeValue) -> c_long;
type Set = extern "C" fn(u64, *const c_char, *const NativeValue) -> c_long;

#[repr(C)]
struct Accessor {
    name: *const c_char,
    get: Option<Get>,
    set: Option<Set>,
    parameter: u64,
}

#[repr(C)]
struct Options {
    size: u32,
    threaded: i8,
    flags: u32,
}

#[repr(C)]
struct Node {
    size: u32,
    kind: i32,
    next: *const Node,
    children: *const Node,
    id: *const c_char,
    label: *const c_char,
    description: *const c_char,
}

type Create = unsafe extern "C" fn(*mut u64, *const c_char, *const Accessor, usize, *const Options) -> c_long;
type Close = unsafe extern "C" fn(u64) -> c_long;
type Write = unsafe extern "C" fn(u64, *const c_char, *const NativeValue) -> c_long;

struct Commands {
    _strings: Vec<(CString, CString)>,
    _nodes: Box<[Node]>,
    roots: Box<[Node; 2]>,
}

impl Commands {
    fn new(commands: &[(&str, &str)]) -> Result<Self, String> {
        let strings = commands
            .iter()
            .map(|(id, label)| Ok((CString::new(*id).map_err(|e| e.to_string())?, CString::new(*label).map_err(|e| e.to_string())?)))
            .collect::<Result<Vec<_>, String>>()?;
        let mut nodes = strings
            .iter()
            .map(|(id, label)| Node {
                size: size_of::<Node>() as u32,
                kind: 2,
                next: std::ptr::null(),
                children: std::ptr::null(),
                id: id.as_ptr(),
                label: label.as_ptr(),
                description: label.as_ptr(),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let first = nodes.as_ptr();
        let count = nodes.len();
        for (i, node) in nodes.iter_mut().enumerate() {
            if i + 1 < count {
                node.next = first.wrapping_add(i + 1);
            }
        }
        let bank = |id: &'static CStr, label: &'static CStr| Node {
            size: size_of::<Node>() as u32,
            kind: 0,
            next: std::ptr::null(),
            children: if count == 0 { std::ptr::null() } else { first },
            id: id.as_ptr(),
            label: label.as_ptr(),
            description: label.as_ptr(),
        };
        // Distinct banks let the user keep button positions while assigning different tools
        // in Design and Sketch. Commands remain available in both; normal enabled checks apply.
        let mut roots = Box::new([bank(c"solidcraft", c"SolidCraft"), bank(c"solidcraft.sketch", c"Sketch")]);
        roots[0].next = std::ptr::from_ref(&roots[1]);
        Ok(Self { _strings: strings, _nodes: nodes, roots })
    }
}

pub(super) struct Connection {
    handle: u64,
    close: Close,
    write: Write,
    _commands: Commands,
    _accessors: Vec<Accessor>,
    _library: Library,
    // The single-threaded driver follows AppKit's run loop. This owner cannot cross threads.
    _main_thread: PhantomData<Rc<()>>,
}

impl Connection {
    pub(super) fn open(path: &str, shared: Arc<Mutex<State>>, commands: &[(&str, &str)]) -> Result<Self, String> {
        // SAFETY: a fixed path to the user's installed driver, never a path from a design file.
        // The three symbols implement the documented C ABI. Their Library owner stays alive
        // through Close, including every callback, in this !Send/!Sync connection.
        let (library, create, close, write) = unsafe {
            let library = Library::new(path).map_err(|e| e.to_string())?;
            let create = *library.get::<Create>(b"NlCreate\0").map_err(|e| e.to_string())?;
            let close = *library.get::<Close>(b"NlClose\0").map_err(|e| e.to_string())?;
            let write = *library.get::<Write>(b"NlWriteValue\0").map_err(|e| e.to_string())?;
            (library, create, close, write)
        };
        let commands = Commands::new(commands)?;
        let properties: &[(&CStr, bool, bool)] = &[
            (c"coordinateSystem", true, false),
            (c"motion", false, true),
            (c"transaction", false, true),
            (c"view.affine", true, true),
            (c"view.extents", true, true),
            (c"view.perspective", true, false),
            (c"view.fov", true, true),
            (c"view.frustum", true, false),
            (c"view.target", true, true),
            (c"view.rotatable", true, false),
            (c"view.focusDistance", true, false),
            (c"view.constructionPlane", true, false),
            (c"views.front", true, false),
            (c"model.extents", true, false),
            (c"selection.empty", true, false),
            (c"selection.extents", true, false),
            (c"pivot.position", true, true),
            (c"pivot.user", true, false),
            (c"pivot.visible", false, true),
            (c"commands.activeCommand", false, true),
        ];
        let accessors: Vec<_> = properties
            .iter()
            .map(|(name, get, set)| Accessor {
                name: name.as_ptr(),
                get: get.then_some(get_property),
                set: set.then_some(set_property),
                parameter: 0,
            })
            .collect();
        {
            let mut callback = CALLBACK.lock().map_err(|_| "3DxWare callback lock failed")?;
            if callback.is_some() {
                return Err("3DxWare is already connected".into());
            }
            *callback = Some(shared);
        }
        // Matrix rows are right/up/back/eye. Setting column_vectors here makes the library
        // treat translation as a projective row and produces non-affine pan/rotation updates.
        let options = Options { size: size_of::<Options>() as u32, threaded: 0, flags: 0 };
        let mut handle = 0;
        // SAFETY: options/accessors have the documented C layout; callbacks use no borrowed
        // user-data. Accessor names have static lifetimes; the vector is retained until Close.
        let result = unsafe { create(&mut handle, c"SolidCraft".as_ptr(), accessors.as_ptr(), accessors.len(), &options) };
        if result != 0 || handle == 0 {
            if let Ok(mut callback) = CALLBACK.lock() {
                *callback = None;
            }
            return Err(format!("3DxWare connection failed ({result:#x})"));
        }
        let connection = Self { handle, close, write, _commands: commands, _accessors: accessors, _library: library, _main_thread: PhantomData };
        let tree = NativeValue { kind: 9, data: Data { node: connection._commands.roots.as_ptr() } };
        connection.write_value(c"commands.tree", tree);
        connection.focus(false);
        Ok(connection)
    }

    fn write_value(&self, name: &CStr, value: NativeValue) {
        // SAFETY: self owns an open handle and the loaded function. Values/pointers are valid
        // through the call; command tree storage is owned by self for the connection's lifetime.
        let result = unsafe { (self.write)(self.handle, name.as_ptr(), &value) };
        if result != 0 {
            log::warn!("3D mouse: writing {} failed ({result:#x})", name.to_string_lossy());
        } else {
            log::trace!("3D mouse: wrote {}", name.to_string_lossy());
        }
    }

    pub(super) fn focus(&self, focused: bool) {
        if !focused {
            self.interrupt();
        }
        self.write_value(c"focus", NativeValue { kind: 1, data: Data { boolean: u32::from(focused) } });
    }

    pub(super) fn interrupt(&self) {
        self.write_value(c"motion", NativeValue { kind: 1, data: Data { boolean: 0 } });
    }

    pub(super) fn command_set(&self, sketch: bool) {
        let id = if sketch { c"solidcraft.sketch" } else { c"solidcraft" };
        self.write_value(
            c"commands.activeSet",
            NativeValue { kind: 8, data: Data { text: Text { pointer: id.as_ptr(), length: id.to_bytes().len() } } },
        );
    }

    pub(super) fn up_axis(&self, coordinates: Matrix, front: Matrix) {
        self.write_value(c"coordinateSystem", NativeValue { kind: 7, data: Data { matrix: coordinates } });
        self.write_value(c"views.front", NativeValue { kind: 7, data: Data { matrix: front } });
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: one successful Create owns exactly one Close, on the same thread, before
        // releasing accessor/command storage or unloading the library. Close ends callbacks.
        unsafe { (self.close)(self.handle) };
        if let Ok(mut callback) = CALLBACK.lock() {
            *callback = None;
        }
    }
}

fn shared() -> Option<Arc<Mutex<State>>> {
    CALLBACK.lock().ok()?.clone()
}

// The driver's C callback contract guarantees a NUL-terminated property name and a correctly
// aligned, live value record during each call. Null pointers and unsupported variant tags are
// rejected. Catch unwinds so a Rust failure never crosses the foreign stack.
extern "C" fn get_property(_parameter: u64, name: *const c_char, out: *mut NativeValue) -> c_long {
    std::panic::catch_unwind(|| {
        if name.is_null() || out.is_null() {
            return UNSUPPORTED;
        }
        // SAFETY: the callback contract above; only the documented variant selected by kind is
        // written. No pointers into Rust state are exposed by a getter.
        unsafe {
            let Ok(name) = CStr::from_ptr(name).to_str() else { return UNSUPPORTED };
            let Some(shared) = shared() else { return UNSUPPORTED };
            let Ok(state) = shared.lock() else { return UNSUPPORTED };
            let Some(value) = state.get(name) else {
                log::trace!("3D mouse: unsupported get {name}");
                return UNSUPPORTED;
            };
            let native = match value {
                Value::Bool(b) => NativeValue { kind: 1, data: Data { boolean: u32::from(b) } },
                Value::Long(l) => NativeValue { kind: 2, data: Data { integer: l } },
                Value::Double(d) => NativeValue { kind: 4, data: Data { number: d } },
                Value::Point(p) => NativeValue { kind: 5, data: Data { point: p } },
                Value::Matrix(m) => NativeValue { kind: 7, data: Data { matrix: m } },
                Value::Plane(p) => NativeValue { kind: 10, data: Data { plane: p } },
                Value::Box(b) => NativeValue { kind: 11, data: Data { bounds: b } },
                Value::Frustum(f) => NativeValue { kind: 12, data: Data { frustum: f } },
                Value::Text(_) => return UNSUPPORTED,
            };
            out.write(native);
        }
        0
    })
    .unwrap_or(UNSUPPORTED)
}

extern "C" fn set_property(_parameter: u64, name: *const c_char, input: *const NativeValue) -> c_long {
    std::panic::catch_unwind(|| {
        if name.is_null() || input.is_null() {
            return UNSUPPORTED;
        }
        // SAFETY: callback pointers are live for this call. Read only the union member named
        // by kind; copy bounded string bytes before returning to the driver.
        unsafe {
            let Ok(name) = CStr::from_ptr(name).to_str() else { return UNSUPPORTED };
            let input = input.read();
            let value = match input.kind {
                1 => Value::Bool(input.data.boolean != 0),
                2 => Value::Long(input.data.integer),
                4 => Value::Double(input.data.number),
                5 => Value::Point(input.data.point),
                7 => Value::Matrix(input.data.matrix),
                8 | 13 => {
                    let text = input.data.text;
                    if text.length > 512 || (text.pointer.is_null() && text.length != 0) {
                        return UNSUPPORTED;
                    }
                    let bytes = if text.length == 0 { &[] } else { std::slice::from_raw_parts(text.pointer.cast::<u8>(), text.length) };
                    let Ok(text) = std::str::from_utf8(bytes) else { return UNSUPPORTED };
                    Value::Text(text.trim_end_matches('\0').to_string())
                }
                11 => Value::Box(input.data.bounds),
                _ => return UNSUPPORTED,
            };
            let Some(shared) = shared() else { return UNSUPPORTED };
            let Ok(mut state) = shared.lock() else { return UNSUPPORTED };
            let accepted = state.set(name, value);
            log::trace!("3D mouse: set {name}, accepted={accepted}");
            if accepted { 0 } else { UNSUPPORTED }
        }
    })
    .unwrap_or(UNSUPPORTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_layouts_match_the_macos_c_abi() {
        assert_eq!(size_of::<NativeValue>(), 136);
        assert_eq!(size_of::<Data>(), 128);
        assert_eq!(align_of::<Data>(), 8);
        assert_eq!(size_of::<Options>(), 12);
        assert_eq!(size_of::<Accessor>(), 32);
        assert_eq!(size_of::<Node>(), 48);
        assert_eq!(std::mem::offset_of!(NativeValue, data), 8);
    }

    #[test]
    fn callbacks_reject_null_without_touching_driver_or_app() {
        assert_eq!(get_property(0, std::ptr::null(), std::ptr::null_mut()), UNSUPPORTED);
        assert_eq!(set_property(0, std::ptr::null(), std::ptr::null()), UNSUPPORTED);
    }
}
