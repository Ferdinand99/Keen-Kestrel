//! Native (Apple and other) event symbolication with SymCaches.
//!
//! Each frame's address is matched to its image in `debug_meta.images`
//! (`image_addr`, else the image whose range holds it); the image's debug
//! id (`debug_id`, or `uuid` from older SDKs) names the SymCache. A return
//! address is moved back into its call instruction
//! (`symbolic_common::InstructionInfo`) unless it is the crashing frame. The
//! lookup gives the function, file and line, innermost inlined function
//! first; the frame becomes the outer function followed by its inlined
//! callees, in Sentry's caller-first order. Swift names are demangled to the
//! form Sentry shows (`CrashPipeline.run(CrashKind)`).
//!
//! Frames of images without a SymCache stay as the SDK sent them, so system
//! frames keep the names sentry-cocoa resolved on the device. A symbolicated
//! frame has `data.symbolicator_status: "symbolicated"`, an inlined one also
//! `inlined: true`; the SDK's own name, if it had one, moves to
//! `raw_function`.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use symbolic_common::{Arch, ByteView, DebugId, InstructionInfo};
use symbolic_demangle::{Demangle, DemangleOptions};
use symbolic_symcache::SymCache;

use crate::event::grouping::{address, basename};

/// The status Sentry gives a frame resolved from a debug file.
pub const SYMBOLICATED: &str = "symbolicated";

/// One image of `debug_meta.images`.
#[derive(Debug, Clone)]
pub struct Image {
    pub debug_id: Option<DebugId>,
    pub addr: u64,
    pub size: u64,
}

/// The images an event lists.
pub fn images(event: &Map<String, Value>) -> Vec<Image> {
    let Some(list) = event
        .get("debug_meta")
        .and_then(|d| d.get("images"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    list.iter()
        .filter_map(Value::as_object)
        .filter_map(|img| {
            let debug_id = img
                .get("debug_id")
                .or_else(|| img.get("uuid"))
                .and_then(Value::as_str)
                .and_then(super::parse_debug_id);
            Some(Image {
                debug_id,
                addr: img.get("image_addr").and_then(address)?,
                size: img.get("image_size").and_then(address).unwrap_or(0),
            })
        })
        .collect()
}

/// The image a frame belongs to: by its `image_addr`, else by range.
pub fn image_of<'a>(images: &'a [Image], frame: &Map<String, Value>) -> Option<&'a Image> {
    let addr = frame.get("instruction_addr").and_then(address)?;
    let frame_image = frame.get("image_addr").and_then(address).filter(|a| *a > 0);
    images
        .iter()
        .find(|im| Some(im.addr) == frame_image)
        .or_else(|| {
            images
                .iter()
                .find(|im| addr >= im.addr && (im.size == 0 || addr < im.addr + im.size))
        })
}

/// The stack traces of an event: `(path to the frames, the crashing stack)`.
fn holders(event: &Map<String, Value>) -> Vec<(&'static str, usize, bool)> {
    let mut out = Vec::new();
    for key in ["exception", "threads"] {
        let Some(values) = event
            .get(key)
            .and_then(|v| v.get("values"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for (i, v) in values.iter().enumerate() {
            let has_frames = v
                .get("stacktrace")
                .and_then(|s| s.get("frames"))
                .and_then(Value::as_array)
                .is_some_and(|f| !f.is_empty());
            if has_frames {
                let crashed = if key == "exception" {
                    v.pointer("/mechanism/handled").and_then(Value::as_bool) == Some(false)
                } else {
                    v.get("crashed").and_then(Value::as_bool) == Some(true)
                };
                out.push((key, i, crashed));
            }
        }
    }
    out
}

/// Whether the event has frames and an image `has` a SymCache for.
pub fn wants(event: &Map<String, Value>, has: impl Fn(&DebugId) -> bool) -> bool {
    !holders(event).is_empty()
        && images(event)
            .iter()
            .any(|i| i.debug_id.as_ref().is_some_and(&has))
}

/// What symbolication did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// Frames looked at (with an address).
    pub frames: usize,
    /// Frames resolved from a SymCache (before inlined ones were added).
    pub symbolicated: usize,
    /// The debug ids whose SymCache resolved a frame.
    pub used: Vec<DebugId>,
}

/// A demangled function name in the form Sentry shows: Swift with
/// parameter types and without the module and return type.
pub fn display_name(name: &symbolic_common::Name<'_>) -> String {
    name.try_demangle(DemangleOptions::complete().return_type(false))
        .into_owned()
}

/// Symbolicates every stack trace of `event` in place, with the SymCaches
/// `caches` finds by debug id.
pub fn symbolicate(
    event: &mut Map<String, Value>,
    caches: &dyn Fn(&DebugId) -> Option<Arc<ByteView<'static>>>,
) -> Report {
    let images = images(event);
    let mut report = Report::default();
    let mut views: HashMap<DebugId, Arc<ByteView<'static>>> = HashMap::new();
    for id in images.iter().filter_map(|i| i.debug_id) {
        if let std::collections::hash_map::Entry::Vacant(slot) = views.entry(id)
            && let Some(view) = caches(&id)
        {
            slot.insert(view);
        }
    }
    if views.is_empty() {
        return report;
    }
    let parsed: HashMap<DebugId, SymCache<'_>> = views
        .iter()
        .filter_map(|(id, v)| SymCache::parse(v).ok().map(|c| (*id, c)))
        .collect();
    let signal = event
        .get("exception")
        .and_then(|e| e.pointer("/values/0/mechanism/meta/signal/number"))
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok());
    let device_arch = event
        .get("contexts")
        .and_then(|c| c.pointer("/device/arch"))
        .and_then(Value::as_str)
        .and_then(|a| Arch::from_str(a).ok());
    let mut used: Vec<DebugId> = Vec::new();
    for (key, index, _crashed) in holders(event) {
        let Some(st) = event
            .get_mut(key)
            .and_then(|v| v.get_mut("values"))
            .and_then(|v| v.get_mut(index))
            .and_then(|v| v.get_mut("stacktrace"))
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        let pc = st
            .get("registers")
            .and_then(|r| r.get("pc").or_else(|| r.get("rip")))
            .and_then(address);
        let Some(Value::Array(frames)) = st.get_mut("frames") else {
            continue;
        };
        let last = frames.len().saturating_sub(1);
        let mut out: Vec<Value> = Vec::with_capacity(frames.len());
        for (i, frame) in std::mem::take(frames).into_iter().enumerate() {
            let Value::Object(frame) = frame else {
                out.push(frame);
                continue;
            };
            let Some(addr) = frame.get("instruction_addr").and_then(address) else {
                out.push(Value::Object(frame));
                continue;
            };
            report.frames += 1;
            let image = image_of(&images, &frame);
            let cache = image
                .and_then(|im| im.debug_id)
                .and_then(|id| parsed.get(&id).map(|c| (id, c)));
            let (Some(image), Some((debug_id, cache))) = (image, cache) else {
                out.push(Value::Object(frame));
                continue;
            };
            let arch = match cache.arch() {
                Arch::Unknown => device_arch.unwrap_or(Arch::Arm64),
                a => a,
            };
            let crashing = i == last;
            let caller = InstructionInfo::new(arch, addr)
                .is_crashing_frame(crashing)
                .signal(signal)
                .ip_register_value(if crashing { pc } else { None })
                .caller_address();
            // SymCache addresses are relative to the object's own load
            // address, so the offset into the loaded image is what is looked up.
            let rel = caller.wrapping_sub(image.addr);
            let locations: Vec<_> = cache.lookup(rel).collect();
            if locations.is_empty() {
                out.push(Value::Object(frame));
                continue;
            }
            report.symbolicated += 1;
            if !used.contains(&debug_id) {
                used.push(debug_id);
            }
            let sdk_name = frame
                .get("function")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|f| !f.is_empty() && f != "<redacted>" && f != "?");
            let n = locations.len();
            // lookup() gives the innermost (inlined) function first.
            for (j, loc) in locations.iter().enumerate().rev() {
                let func = loc.function();
                let mut f = frame.clone();
                f.insert(
                    "function".into(),
                    Value::from(display_name(&func.name_for_demangling())),
                );
                f.insert("symbol".into(), Value::from(func.name()));
                match loc.file().map(|file| file.full_path()) {
                    Some(path) if !path.is_empty() => {
                        f.insert("filename".into(), Value::from(basename(&path)));
                        f.insert("abs_path".into(), Value::from(path));
                    }
                    _ => {
                        f.remove("filename");
                        f.remove("abs_path");
                    }
                }
                match loc.line() {
                    0 => f.remove("lineno"),
                    line => f.insert("lineno".into(), Value::from(line)),
                };
                let inlined = j + 1 < n;
                if inlined {
                    f.insert("inlined".into(), Value::Bool(true));
                } else {
                    f.remove("inlined");
                    f.insert(
                        "symbol_addr".into(),
                        Value::from(format!(
                            "{:#x}",
                            image.addr.wrapping_add(u64::from(func.entry_pc()))
                        )),
                    );
                    if let Some(name) = &sdk_name {
                        f.insert("raw_function".into(), Value::from(name.clone()));
                    }
                }
                let data = f.entry("data").or_insert_with(|| Value::Object(Map::new()));
                if let Value::Object(d) = data {
                    d.insert("symbolicator_status".into(), json!(SYMBOLICATED));
                }
                out.push(Value::Object(f));
            }
        }
        *frames = out;
    }
    report.used = used;
    report
}

/// Whether a stored frame was symbolicated by Kestrel.
pub fn is_symbolicated(frame: &Map<String, Value>) -> bool {
    frame
        .get("data")
        .and_then(|d| d.get("symbolicator_status"))
        .and_then(Value::as_str)
        == Some(SYMBOLICATED)
}

#[cfg(test)]
mod tests {
    use symbolic_common::{Language, Name, NameMangling};

    use super::*;

    #[test]
    fn swift_names_demangle_to_the_shown_form() {
        let name = Name::new(
            "$s8mangling9r137577441xySaySiG_tF",
            NameMangling::Mangled,
            Language::Swift,
        );
        assert_eq!(display_name(&name), "r13757744(x: [Int])");
    }

    #[test]
    fn frames_without_a_symcache_stay_as_sent() {
        let mut event = json!({
            "debug_meta": {"images": [
                {"type": "macho", "debug_id": "8d5d1ec4-0b3c-3d93-a7b3-1f0b5bfb5e3c",
                 "image_addr": "0x100000000", "image_size": 4096, "code_file": "/x/App"}
            ]},
            "threads": {"values": [{"id": 0, "crashed": true, "stacktrace": {"frames": [
                {"instruction_addr": "0x100000010", "function": "main"},
                {"instruction_addr": "0x200000010", "function": "libfoo"}
            ]}}]}
        });
        let map = event.as_object_mut().unwrap();
        assert!(wants(map, |_| true));
        assert!(!wants(map, |_| false));
        let before = map.clone();
        let r = symbolicate(map, &|_| None);
        assert_eq!(r, Report::default());
        assert_eq!(*map, before);
        let images = images(map);
        let frames = map["threads"]["values"][0]["stacktrace"]["frames"]
            .as_array()
            .unwrap();
        let f0 = frames[0].as_object().unwrap();
        assert_eq!(image_of(&images, f0).unwrap().addr, 0x1_0000_0000);
        assert!(image_of(&images, frames[1].as_object().unwrap()).is_none());
        assert!(!is_symbolicated(f0));
    }
}
