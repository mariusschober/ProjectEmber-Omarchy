//! Optional gtk4-layer-shell integration loaded at runtime.
//!
//! Omarchy ships `gtk4-layer-shell`, so the control panel can be a true
//! overlay anchored under the bar instead of a tiled window. When the library
//! or compositor support is absent, Ember falls back to a normal window.

use std::ffi::{c_char, c_void, CString};

use gtk4 as gtk;
use gtk4::glib::translate::ToGlibPtr;

type BoolFn = unsafe extern "C" fn() -> i32;
type WindowFn = unsafe extern "C" fn(*mut c_void);
type NamespaceFn = unsafe extern "C" fn(*mut c_void, *const c_char);
type LayerFn = unsafe extern "C" fn(*mut c_void, i32);
type AnchorFn = unsafe extern "C" fn(*mut c_void, i32, i32);
type MarginFn = unsafe extern "C" fn(*mut c_void, i32, i32);
type KeyboardFn = unsafe extern "C" fn(*mut c_void, i32);

const EDGE_LEFT: i32 = 0;
const EDGE_RIGHT: i32 = 1;
const EDGE_TOP: i32 = 2;
const LAYER_TOP: i32 = 2;
const KEYBOARD_ON_DEMAND: i32 = 2;

pub fn configure(window: &gtk::Window) -> bool {
    unsafe {
        let library = match libloading::Library::new("libgtk4-layer-shell.so.0")
            .or_else(|_| libloading::Library::new("libgtk4-layer-shell.so"))
        {
            Ok(library) => library,
            Err(_) => return false,
        };
        let is_supported = match library.get::<BoolFn>(b"gtk_layer_is_supported\0") {
            Ok(symbol) => symbol,
            Err(_) => return false,
        };
        if is_supported() == 0 {
            return false;
        }
        let init = match library.get::<WindowFn>(b"gtk_layer_init_for_window\0") {
            Ok(symbol) => symbol,
            Err(_) => return false,
        };
        let set_namespace = match library.get::<NamespaceFn>(b"gtk_layer_set_namespace\0") {
            Ok(symbol) => symbol,
            Err(_) => return false,
        };
        let set_layer = match library.get::<LayerFn>(b"gtk_layer_set_layer\0") {
            Ok(symbol) => symbol,
            Err(_) => return false,
        };
        let set_anchor = match library.get::<AnchorFn>(b"gtk_layer_set_anchor\0") {
            Ok(symbol) => symbol,
            Err(_) => return false,
        };
        let set_margin = match library.get::<MarginFn>(b"gtk_layer_set_margin\0") {
            Ok(symbol) => symbol,
            Err(_) => return false,
        };
        let set_keyboard = match library.get::<KeyboardFn>(b"gtk_layer_set_keyboard_mode\0") {
            Ok(symbol) => symbol,
            Err(_) => return false,
        };

        let pointer: *mut gtk::ffi::GtkWindow = window.to_glib_none().0;
        let pointer = pointer as *mut c_void;
        init(pointer);
        let namespace = CString::new("project-ember").unwrap();
        set_namespace(pointer, namespace.as_ptr());
        set_layer(pointer, LAYER_TOP);
        set_anchor(pointer, EDGE_TOP, 1);
        set_anchor(pointer, EDGE_RIGHT, 1);
        set_anchor(pointer, EDGE_LEFT, 0);
        set_anchor(pointer, 3, 0); // bottom
        set_margin(pointer, EDGE_TOP, 44);
        set_margin(pointer, EDGE_RIGHT, 12);
        set_keyboard(pointer, KEYBOARD_ON_DEMAND);

        // Keep the library loaded for the lifetime of the window.
        std::mem::forget(library);
        true
    }
}
