//! Plugin editor windows. Each embedded editor gets its own top-level X11 window (created with
//! x11rb, which talks to `$DISPLAY` directly and needs no C headers); the plugin parents its
//! view into it. Plugins that only offer floating windows manage their own.

use super::PluginInstance;
use std::sync::Arc;
use x11rb::connection::Connection;
use x11rb::properties::WmSizeHints;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

struct X11 {
    conn: RustConnection,
    root: Window,
    wm_protocols: Atom,
    wm_delete: Atom,
    net_wm_name: Atom,
    utf8_string: Atom,
}

impl X11 {
    fn connect() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11: {e}"))?;
        let root = conn.setup().roots[screen].root;
        let atom = |name: &[u8]| -> Result<Atom, String> {
            Ok(conn
                .intern_atom(false, name)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?
                .atom)
        };
        Ok(Self {
            wm_protocols: atom(b"WM_PROTOCOLS")?,
            wm_delete: atom(b"WM_DELETE_WINDOW")?,
            net_wm_name: atom(b"_NET_WM_NAME")?,
            utf8_string: atom(b"UTF8_STRING")?,
            root,
            conn,
        })
    }

    fn create_window(&self, title: &str, w: u32, h: u32, resizable: bool) -> Result<Window, String> {
        let c = &self.conn;
        let err = |e: x11rb::errors::ConnectionError| e.to_string();
        let win = c.generate_id().map_err(|e| e.to_string())?;
        c.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            win,
            self.root,
            0,
            0,
            w.max(1) as u16,
            h.max(1) as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .background_pixel(bg_pixel())
                .event_mask(EventMask::STRUCTURE_NOTIFY),
        )
        .map_err(err)?;
        c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())
            .map_err(err)?;
        c.change_property8(PropMode::REPLACE, win, self.net_wm_name, self.utf8_string, title.as_bytes())
            .map_err(err)?;
        c.change_property32(PropMode::REPLACE, win, self.wm_protocols, AtomEnum::ATOM, &[self.wm_delete])
            .map_err(err)?;
        c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"rasdaw-plugin\0RasDaw\0")
            .map_err(err)?;
        if !resizable {
            let mut hints = WmSizeHints::new();
            hints.min_size = Some((w as i32, h as i32));
            hints.max_size = Some((w as i32, h as i32));
            hints.set_normal_hints(c, win).map_err(err)?;
        }
        c.map_window(win).map_err(err)?;
        c.flush().map_err(err)?;
        Ok(win)
    }

    fn resize(&self, win: Window, w: u32, h: u32) {
        let _ = self
            .conn
            .configure_window(win, &ConfigureWindowAux::new().width(w).height(h));
        let _ = self.conn.flush();
    }

    fn destroy(&self, win: Window) {
        let _ = self.conn.destroy_window(win);
        let _ = self.conn.flush();
    }
}

/// Window background (theme colour as a 24-bit TrueColor pixel), visible until the plugin draws.
fn bg_pixel() -> u32 {
    let bg = crate::theme::current().bg;
    ((bg.r() as u32) << 16) | ((bg.g() as u32) << 8) | bg.b() as u32
}

struct EditorWindow {
    instance: Arc<PluginInstance>,
    /// None for floating editors.
    window: Option<Window>,
    size: (u32, u32),
    resizable: bool,
}

#[derive(Default)]
pub struct GuiManager {
    x11: Option<X11>,
    windows: Vec<EditorWindow>,
}

impl GuiManager {
    pub fn is_open(&self, uid: u64) -> bool {
        self.windows.iter().any(|w| w.instance.uid == uid)
    }

    pub fn toggle(&mut self, instance: &Arc<PluginInstance>, title: &str) -> Result<(), String> {
        if self.is_open(instance.uid) {
            self.close(instance.uid);
            Ok(())
        } else {
            self.open(instance, title)
        }
    }

    pub fn open(&mut self, instance: &Arc<PluginInstance>, title: &str) -> Result<(), String> {
        if self.is_open(instance.uid) {
            return Ok(());
        }
        if !instance.has_gui() {
            return Err(format!("{} has no editor", instance.desc.name));
        }
        if instance.gui_is_api_supported(false) {
            if self.x11.is_none() {
                self.x11 = Some(X11::connect()?);
            }
            let x11 = self.x11.as_ref().unwrap();
            if !instance.gui_create(false) {
                return Err("gui.create failed".into());
            }
            instance.gui_set_scale(1.0);
            let (w, h) = instance.gui_get_size().unwrap_or((640, 480));
            let resizable = instance.gui_can_resize();
            let win = match x11.create_window(title, w, h, resizable) {
                Ok(win) => win,
                Err(e) => {
                    instance.gui_destroy();
                    return Err(e);
                }
            };
            if !instance.gui_set_parent(win) {
                instance.gui_destroy();
                x11.destroy(win);
                return Err("gui.set_parent failed".into());
            }
            instance.gui_show();
            self.windows.push(EditorWindow {
                instance: instance.clone(),
                window: Some(win),
                size: (w, h),
                resizable,
            });
            Ok(())
        } else if instance.gui_is_api_supported(true) {
            if !instance.gui_create(true) {
                return Err("gui.create (floating) failed".into());
            }
            instance.gui_suggest_title(title);
            instance.gui_show();
            self.windows.push(EditorWindow {
                instance: instance.clone(),
                window: None,
                size: (0, 0),
                resizable: false,
            });
            Ok(())
        } else {
            Err("the plugin supports no X11 editor".into())
        }
    }

    pub fn close(&mut self, uid: u64) {
        if let Some(i) = self.windows.iter().position(|w| w.instance.uid == uid) {
            let w = self.windows.remove(i);
            self.teardown(w);
        }
    }

    pub fn close_all(&mut self) {
        for w in std::mem::take(&mut self.windows) {
            self.teardown(w);
        }
    }

    /// Destroys the plugin view before its parent window, so the plugin never sees its
    /// parent disappear underneath it.
    fn teardown(&self, w: EditorWindow) {
        w.instance.gui_hide();
        w.instance.gui_destroy();
        if let (Some(win), Some(x11)) = (w.window, &self.x11) {
            x11.destroy(win);
        }
    }

    /// Call once per UI frame: handles window-manager close buttons, user resizes and resize
    /// requests from plugins.
    pub fn poll(&mut self) {
        let mut to_close: Vec<u64> = Vec::new();
        if let Some(x11) = &self.x11 {
            while let Ok(Some(event)) = x11.conn.poll_for_event() {
                match event {
                    Event::ClientMessage(e)
                        if e.format == 32 && e.data.as_data32()[0] == x11.wm_delete =>
                    {
                        if let Some(w) = self.windows.iter().find(|w| w.window == Some(e.window)) {
                            to_close.push(w.instance.uid);
                        }
                    }
                    Event::ConfigureNotify(e) => {
                        let size = (e.width as u32, e.height as u32);
                        if let Some(w) = self
                            .windows
                            .iter_mut()
                            .find(|w| w.window == Some(e.window) && w.size != size)
                        {
                            if w.resizable {
                                let adjusted = w.instance.gui_adjust_size(size.0, size.1);
                                w.instance.gui_set_size(adjusted.0, adjusted.1);
                                w.size = adjusted;
                                if adjusted != size {
                                    x11.resize(e.window, adjusted.0, adjusted.1);
                                }
                            } else {
                                // Not resizable: snap back.
                                x11.resize(e.window, w.size.0, w.size.1);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for w in &mut self.windows {
            if let Some((rw, rh)) = w.instance.take_gui_resize_request() {
                if let (Some(win), Some(x11)) = (w.window, &self.x11) {
                    if !w.resizable {
                        let mut hints = WmSizeHints::new();
                        hints.min_size = Some((rw as i32, rh as i32));
                        hints.max_size = Some((rw as i32, rh as i32));
                        let _ = hints.set_normal_hints(&x11.conn, win);
                    }
                    w.size = (rw, rh);
                    x11.resize(win, rw, rh);
                }
            }
            if w.instance.take_gui_closed().is_some() {
                to_close.push(w.instance.uid);
            }
        }
        for uid in to_close {
            self.close(uid);
        }
    }
}

impl Drop for GuiManager {
    fn drop(&mut self) {
        self.close_all();
    }
}
