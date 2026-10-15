//! Finds `.clap` files, loads them and lists the plugins they contain.

use super::cstr_to_string;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A loaded `.clap` library. It stays loaded (and initialised) as long as any descriptor or
/// instance refers to it.
pub struct PluginLibrary {
    pub path: PathBuf,
    entry: *const clap_plugin_entry,
    pub(crate) factory: *const clap_plugin_factory,
    _lib: libloading::Library,
}

// The entry and factory are plain function tables owned by the library.
unsafe impl Send for PluginLibrary {}
unsafe impl Sync for PluginLibrary {}

impl Drop for PluginLibrary {
    fn drop(&mut self) {
        unsafe {
            if let Some(deinit) = (*self.entry).deinit {
                deinit();
            }
        }
    }
}

impl PluginLibrary {
    pub fn load(path: &Path) -> Result<Arc<Self>, String> {
        unsafe {
            let lib = libloading::Library::new(path).map_err(|e| e.to_string())?;
            let entry: *const clap_plugin_entry = *lib
                .get::<*const clap_plugin_entry>(b"clap_entry\0")
                .map_err(|e| format!("no clap_entry: {e}"))?;
            if entry.is_null() {
                return Err("clap_entry is null".into());
            }
            let cpath = CString::new(path.to_string_lossy().as_bytes()).unwrap_or_default();
            let init = (*entry).init.ok_or("entry has no init")?;
            if !init(cpath.as_ptr()) {
                return Err("clap_entry.init failed".into());
            }
            let get_factory = (*entry).get_factory.ok_or("entry has no get_factory")?;
            let factory =
                get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
            if factory.is_null() {
                if let Some(deinit) = (*entry).deinit {
                    deinit();
                }
                return Err("no plugin factory".into());
            }
            Ok(Arc::new(Self {
                path: path.to_owned(),
                entry,
                factory,
                _lib: lib,
            }))
        }
    }

    pub fn descriptors(self: &Arc<Self>) -> Vec<PluginDesc> {
        let mut out = Vec::new();
        unsafe {
            let f = &*self.factory;
            let (Some(count), Some(get)) = (f.get_plugin_count, f.get_plugin_descriptor) else {
                return out;
            };
            for i in 0..count(self.factory) {
                let d = get(self.factory, i);
                if d.is_null() {
                    continue;
                }
                let d = &*d;
                let mut features = Vec::new();
                if !d.features.is_null() {
                    let mut p = d.features;
                    while !(*p).is_null() {
                        features.push(cstr_to_string(*p));
                        p = p.add(1);
                    }
                }
                out.push(PluginDesc {
                    id: cstr_to_string(d.id),
                    name: cstr_to_string(d.name),
                    vendor: cstr_to_string(d.vendor),
                    version: cstr_to_string(d.version),
                    features,
                    library: self.clone(),
                });
            }
        }
        out
    }
}

#[derive(Clone)]
pub struct PluginDesc {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub features: Vec<String>,
    pub library: Arc<PluginLibrary>,
}

impl PluginDesc {
    pub fn is_instrument(&self) -> bool {
        self.features.iter().any(|f| f == "instrument")
    }

    /// Note-altering plugin (arpeggiator, chord generator, ...).
    pub fn is_note_effect(&self) -> bool {
        !self.is_instrument() && self.features.iter().any(|f| f == "note-effect")
    }

    pub fn is_effect(&self) -> bool {
        self.features
            .iter()
            .any(|f| f == "audio-effect" || f == "analyzer")
    }
}

#[derive(Default)]
pub struct Registry {
    pub plugins: Vec<PluginDesc>,
    /// Files that failed to load, with the reason.
    pub errors: Vec<(PathBuf, String)>,
}

impl Registry {
    /// Standard CLAP search path on Linux: `$CLAP_PATH`, `~/.clap`, `/usr/lib/clap`,
    /// `/usr/local/lib/clap`.
    pub fn search_dirs() -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::env::var("CLAP_PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".clap"));
        }
        dirs.push("/usr/lib/clap".into());
        dirs.push("/usr/local/lib/clap".into());
        dirs
    }

    pub fn scan() -> Self {
        let mut files = Vec::new();
        for dir in Self::search_dirs() {
            collect_clap_files(&dir, 0, &mut files);
        }
        files.sort();
        files.dedup();
        let mut reg = Registry::default();
        for path in files {
            match PluginLibrary::load(&path) {
                Ok(lib) => reg.plugins.extend(lib.descriptors()),
                Err(e) => reg.errors.push((path, e)),
            }
        }
        reg.plugins.sort_by_key(|p| p.name.to_lowercase());
        reg
    }

    pub fn find(&self, id: &str) -> Option<&PluginDesc> {
        self.plugins.iter().find(|p| p.id == id)
    }
}

fn collect_clap_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let is_clap = path.extension().is_some_and(|e| e == "clap");
        // `metadata` follows symlinks, which is how plugins are usually installed into ~/.clap.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_file() && is_clap {
            out.push(path);
        } else if meta.is_dir() && depth < 4 {
            collect_clap_files(&path, depth + 1, out);
        }
    }
}
