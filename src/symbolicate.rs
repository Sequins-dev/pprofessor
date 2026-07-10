//! Symbolication: resolve raw instruction pointer addresses to function names
//! and source locations using DWARF debug info.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use gimli::{EndianSlice, RunTimeEndian};
use memmap2::Mmap;
use object::read::macho::{FatArch, MachOFatFile32, MachOFatFile64};
use object::{Architecture, FileKind, Object, ObjectSection, ObjectSegment, ObjectSymbol};

use crate::sampler::LoadedImage;

/// Binaries larger than this skip DWARF indexing (which is O(compilation units)
/// and hangs on large Rust debug builds). Symbol table resolution still runs.
const MAX_DWARF_BYTES: usize = 50 * 1024 * 1024; // 50 MB

// ---------------------------------------------------------------------------
// Symbolizer trait
// ---------------------------------------------------------------------------

/// Symbolication output for a single address.
#[derive(Debug, Clone)]
pub struct FrameInfo {
    pub function: String,
    pub file: String,
    pub line: u32,
}

/// Resolves raw instruction pointer addresses to stack frames.
///
/// Implement this trait for custom symbolication (e.g. JIT code in a language VM).
/// Platform-specific data (loaded images, JIT metadata, etc.) should be provided
/// at construction time — the trait itself is platform-agnostic.
///
/// Return `None` if this symbolizer doesn't handle the given address.
/// In a chain, the next symbolizer is tried. If ALL symbolizers return `None`,
/// the frame is handled according to the [`Unresolved`](crate::Unresolved) setting.
pub trait Symbolizer: Send + Sync {
    fn symbolize_frame(&self, address: u64) -> Option<FrameInfo>;
}

/// Composes multiple symbolizers. Tries each in order; first `Some` wins.
/// If all return `None`, the address is unresolved.
pub struct SymbolizerChain {
    symbolizers: Vec<Box<dyn Symbolizer>>,
}

impl SymbolizerChain {
    pub fn new(symbolizers: Vec<Box<dyn Symbolizer>>) -> Self {
        Self { symbolizers }
    }
}

impl Symbolizer for SymbolizerChain {
    fn symbolize_frame(&self, address: u64) -> Option<FrameInfo> {
        for s in &self.symbolizers {
            if let Some(info) = s.symbolize_frame(address) {
                return Some(info);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// NativeSymbolizer — DWARF + symbol table
// ---------------------------------------------------------------------------

/// The resolved name and source location for a single address.
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub function: String,
    pub file: String,
    pub line: u32,
}

/// Native symbolizer: resolves addresses using DWARF debug info and symbol tables.
///
/// Constructed with `Vec<LoadedImage>` and a list of addresses to batch-resolve.
/// Returns `None` for addresses that cannot be resolved (e.g. system libraries
/// in the dyld shared cache). Use [`Unresolved`](crate::Unresolved) to control
/// what happens to unresolved frames.
pub struct NativeSymbolizer {
    images: Vec<LoadedImage>,
    resolved: HashMap<u64, SymbolInfo>,
    attempted: HashSet<u64>,
}

impl NativeSymbolizer {
    /// Batch-resolve all `addresses` using DWARF debug info and symbol tables
    /// from the given `images`. Unresolvable addresses are not stored — they
    /// result in `None` from `symbolize_frame`.
    pub fn new(images: Vec<LoadedImage>, addresses: &[u64]) -> Self {
        let resolved = resolve_all(&images, addresses);
        Self {
            images,
            resolved,
            attempted: addresses.iter().copied().collect(),
        }
    }

    /// Resolve addresses not already seen by this session and retain the results.
    pub fn resolve_more(&mut self, addresses: &[u64]) {
        let pending: Vec<u64> = addresses
            .iter()
            .copied()
            .filter(|address| self.attempted.insert(*address))
            .collect();
        if !pending.is_empty() {
            self.resolved.extend(resolve_all(&self.images, &pending));
        }
    }

    /// Replace the loaded-image set after the target changes and retry addresses
    /// that could not be resolved against the previous set.
    pub fn refresh_images(&mut self, images: Vec<LoadedImage>) {
        if images.is_empty() || images == self.images {
            return;
        }
        self.images = images;
        self.attempted
            .retain(|address| self.resolved.contains_key(address));
    }
}

impl Symbolizer for NativeSymbolizer {
    fn symbolize_frame(&self, address: u64) -> Option<FrameInfo> {
        let info = self.resolved.get(&address)?;
        Some(FrameInfo {
            function: info.function.clone(),
            file: info.file.clone(),
            line: info.line,
        })
    }
}

// ---------------------------------------------------------------------------
// Internal: image mapping and resolution
// ---------------------------------------------------------------------------

/// Internal: a memory-mapped image with its load metadata.
struct ImageInfo {
    load_address: u64,
    /// Preferred virtual address of the __TEXT segment (for computing ASLR slide).
    text_vmaddr: u64,
    mmap: Mmap,
    object_offset: usize,
    object_len: usize,
}

impl ImageInfo {
    fn object_data(&self) -> &[u8] {
        &self.mmap[self.object_offset..self.object_offset + self.object_len]
    }
}

/// Returns true for paths that live in the macOS dyld shared cache rather than
/// as standalone files on disk (macOS 12+). Missing files at these paths are
/// expected and should not produce warnings.
fn is_system_cache_path(path: &str) -> bool {
    path.starts_with("/usr/lib/") || path.starts_with("/System/Library/")
}

/// Resolve a set of addresses to symbolic information using `images`.
/// Only successfully resolved addresses are included in the result.
pub(crate) fn resolve_all(images: &[LoadedImage], addresses: &[u64]) -> HashMap<u64, SymbolInfo> {
    if addresses.is_empty() {
        return HashMap::new();
    }

    let mut sorted_images = images.to_vec();
    sorted_images.sort_by_key(|img| img.load_address);

    // Memory-map each image file.
    let mut image_infos: Vec<ImageInfo> = Vec::new();
    for img in &sorted_images {
        match open_image(&img.path, img.load_address) {
            Ok(info) => image_infos.push(info),
            Err(e) => {
                if !is_system_cache_path(&img.path) {
                    eprintln!("pprofessor: warning: could not open {}: {e}", img.path);
                }
            }
        }
    }
    image_infos.sort_by_key(|i| i.load_address);

    let mut result: HashMap<u64, SymbolInfo> = HashMap::new();
    let mut by_image: Vec<Vec<u64>> = vec![Vec::new(); image_infos.len()];

    for &addr in addresses {
        let idx = image_infos.partition_point(|img| img.load_address <= addr);
        if idx > 0 {
            let img = &image_infos[idx - 1];
            // Reject addresses beyond the end of the image — they belong to JIT
            // code or another region not backed by this file.
            let offset = addr.wrapping_sub(img.load_address);
            if offset < img.object_len as u64 {
                by_image[idx - 1].push(addr);
            }
        }
        // Addresses below all image load addresses are simply not inserted.
    }

    for (img, addrs) in image_infos.iter().zip(by_image.iter()) {
        if addrs.is_empty() {
            continue;
        }
        resolve_for_image(img, addrs, &mut result);
    }

    result
}

fn open_image(path: &str, load_address: u64) -> Result<ImageInfo> {
    let file = std::fs::File::open(path)?;
    let mmap = unsafe { Mmap::map(&file)? };
    let object_data = native_macho_slice(&mmap)?;
    let object_offset = object_data.as_ptr() as usize - mmap.as_ptr() as usize;
    let object_len = object_data.len();
    let text_vmaddr = {
        let obj = object::File::parse(object_data)?;
        text_segment_vmaddr(&obj)
    };
    Ok(ImageInfo {
        load_address,
        text_vmaddr,
        mmap,
        object_offset,
        object_len,
    })
}

fn native_macho_slice(data: &[u8]) -> Result<&[u8]> {
    let native_architecture = if cfg!(target_arch = "aarch64") {
        Architecture::Aarch64
    } else {
        Architecture::X86_64
    };
    match FileKind::parse(data)? {
        FileKind::MachOFat32 => {
            let fat = MachOFatFile32::parse(data)?;
            let arch = fat
                .arches()
                .iter()
                .find(|arch| arch.architecture() == native_architecture)
                .ok_or_else(|| anyhow::anyhow!("universal Mach-O has no native architecture"))?;
            Ok(arch.data(data)?)
        }
        FileKind::MachOFat64 => {
            let fat = MachOFatFile64::parse(data)?;
            let arch = fat
                .arches()
                .iter()
                .find(|arch| arch.architecture() == native_architecture)
                .ok_or_else(|| anyhow::anyhow!("universal Mach-O has no native architecture"))?;
            Ok(arch.data(data)?)
        }
        _ => Ok(data),
    }
}

fn text_segment_vmaddr(obj: &object::File<'_>) -> u64 {
    for seg in obj.segments() {
        if let Some(name) = seg.name().ok().flatten()
            && name == "__TEXT"
        {
            return seg.address();
        }
    }
    obj.segments().map(|s| s.address()).min().unwrap_or(0)
}

fn resolve_for_image(img: &ImageInfo, addrs: &[u64], out: &mut HashMap<u64, SymbolInfo>) {
    let obj = match object::File::parse(img.object_data()) {
        Ok(o) => o,
        Err(_) => return, // can't open — addresses simply won't be in the result
    };

    let ctx = if img.object_len <= MAX_DWARF_BYTES {
        build_context(&obj)
    } else {
        None
    };

    for &addr in addrs {
        let file_addr = addr
            .wrapping_sub(img.load_address)
            .wrapping_add(img.text_vmaddr);

        let sym = if let Some(ref ctx) = ctx {
            resolve_with_ctx(ctx, file_addr, addr)
                .or_else(|| resolve_via_symbol_table(&obj, file_addr))
        } else {
            resolve_via_symbol_table(&obj, file_addr)
        };

        if let Some(info) = sym {
            out.insert(addr, info);
        }
        // No fallback — unresolvable addresses are simply absent from the result.
    }
}

/// Build an addr2line context using zero-copy slices borrowed from the mmap.
/// Returns None if the binary has no usable DWARF debug info.
fn build_context<'a>(
    obj: &object::File<'a>,
) -> Option<addr2line::Context<EndianSlice<'a, RunTimeEndian>>> {
    let endian = if obj.is_little_endian() {
        RunTimeEndian::Little
    } else {
        RunTimeEndian::Big
    };

    let load_section = |id: gimli::SectionId| -> gimli::Result<EndianSlice<'a, RunTimeEndian>> {
        let elf_name = id.name();
        let macho_name = format!("__{}", &elf_name[1..]);
        let data = obj
            .section_by_name(elf_name)
            .or_else(|| obj.section_by_name(&macho_name))
            .and_then(|s| s.data().ok())
            .unwrap_or(&[]);
        Ok(EndianSlice::new(data, endian))
    };

    let dwarf = gimli::Dwarf::load(load_section).ok()?;
    addr2line::Context::from_dwarf(dwarf).ok()
}

fn resolve_with_ctx<R: gimli::Reader>(
    ctx: &addr2line::Context<R>,
    file_addr: u64,
    orig_addr: u64,
) -> Option<SymbolInfo> {
    let mut frames = ctx.find_frames(file_addr).skip_all_loads().ok()?;
    let frame = frames.next().ok()??;

    let function = frame
        .function
        .as_ref()
        .and_then(|f: &addr2line::FunctionName<R>| f.raw_name().ok())
        .map(|n: std::borrow::Cow<'_, str>| demangle(n.as_ref()))
        .unwrap_or_else(|| format!("0x{orig_addr:016x}"));

    let (file, line) = frame
        .location
        .as_ref()
        .map(|loc| (loc.file.unwrap_or("").to_string(), loc.line.unwrap_or(0)))
        .unwrap_or_default();

    Some(SymbolInfo {
        function,
        file,
        line,
    })
}

fn resolve_via_symbol_table(obj: &object::File<'_>, file_addr: u64) -> Option<SymbolInfo> {
    let mut best: Option<(u64, String)> = None;

    for sym in obj.symbols() {
        let sym_addr = sym.address();
        if sym_addr == 0 || sym_addr > file_addr {
            continue;
        }
        let name = sym.name().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        match &best {
            None => best = Some((sym_addr, name)),
            Some((prev_addr, _)) if sym_addr > *prev_addr => {
                best = Some((sym_addr, name));
            }
            _ => {}
        }
    }

    best.map(|(_, name)| SymbolInfo {
        function: demangle(&name),
        file: String::new(),
        line: 0,
    })
}

fn demangle(name: &str) -> String {
    symbolic_demangle::demangle(name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_all_empty() {
        let result = resolve_all(&[], &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_resolve_all_no_images_returns_empty() {
        // Without any images, no address can be resolved.
        let result = resolve_all(&[], &[0x1000u64]);
        assert!(
            result.is_empty(),
            "expected empty result for address with no images"
        );
    }

    #[test]
    fn test_native_symbolizer_unknown_returns_none() {
        let sym = NativeSymbolizer::new(vec![], &[0x1000u64]);
        assert!(sym.symbolize_frame(0x1000).is_none());
    }

    #[test]
    fn test_native_symbolizer_can_resolve_incremental_batches() {
        let mut sym = NativeSymbolizer::new(vec![], &[]);
        sym.resolve_more(&[0x1000u64, 0x2000u64]);
        assert!(sym.symbolize_frame(0x1000).is_none());
    }

    #[test]
    fn test_native_symbolizer_retries_unresolved_addresses_after_image_refresh() {
        let path = std::env::current_exe().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let object = object::File::parse(&*bytes).unwrap();
        let text_vmaddr = text_segment_vmaddr(&object);
        let symbol = object
            .symbols()
            .find(|symbol| {
                symbol.address() >= text_vmaddr
                    && symbol.address() - text_vmaddr < bytes.len() as u64
                    && symbol.name().is_ok_and(|name| !name.is_empty())
            })
            .expect("test executable should contain a resolvable symbol");
        let load_address = 0x10_0000_0000;
        let runtime_address = load_address + symbol.address() - text_vmaddr;
        let mut symbolizer = NativeSymbolizer::new(vec![], &[runtime_address]);
        assert!(symbolizer.symbolize_frame(runtime_address).is_none());

        symbolizer.refresh_images(vec![LoadedImage {
            load_address,
            path: path.to_string_lossy().into_owned(),
        }]);
        symbolizer.resolve_more(&[runtime_address]);

        assert!(symbolizer.symbolize_frame(runtime_address).is_some());
    }

    #[test]
    fn test_native_symbolizer_resolves_universal_dyld_image() {
        let path = "/usr/lib/dyld";
        let bytes = std::fs::read(path).unwrap();
        let object_data = native_macho_slice(&bytes).unwrap();
        let object = object::File::parse(object_data).unwrap();
        let text_vmaddr = text_segment_vmaddr(&object);
        let start = object
            .symbols()
            .find(|symbol| symbol.name().is_ok_and(|name| name == "start"))
            .expect("native dyld slice should contain start");
        let load_address = 0x10_0000_0000;
        let runtime_address = load_address + start.address() - text_vmaddr;

        let symbolizer = NativeSymbolizer::new(
            vec![LoadedImage {
                load_address,
                path: path.to_string(),
            }],
            &[runtime_address],
        );

        assert_eq!(
            symbolizer
                .symbolize_frame(runtime_address)
                .unwrap()
                .function,
            "start"
        );
    }

    #[test]
    fn test_symbolizer_chain_first_some_wins() {
        struct AlwaysFoo;
        impl Symbolizer for AlwaysFoo {
            fn symbolize_frame(&self, _: u64) -> Option<FrameInfo> {
                Some(FrameInfo {
                    function: "foo".to_string(),
                    file: String::new(),
                    line: 0,
                })
            }
        }
        struct AlwaysBar;
        impl Symbolizer for AlwaysBar {
            fn symbolize_frame(&self, _: u64) -> Option<FrameInfo> {
                Some(FrameInfo {
                    function: "bar".to_string(),
                    file: String::new(),
                    line: 0,
                })
            }
        }
        let chain = SymbolizerChain::new(vec![Box::new(AlwaysFoo), Box::new(AlwaysBar)]);
        let result = chain.symbolize_frame(0x1000).unwrap();
        assert_eq!(result.function, "foo");
    }

    #[test]
    fn test_symbolizer_chain_fallback() {
        struct AlwaysNone;
        impl Symbolizer for AlwaysNone {
            fn symbolize_frame(&self, _: u64) -> Option<FrameInfo> {
                None
            }
        }
        struct AlwaysBaz;
        impl Symbolizer for AlwaysBaz {
            fn symbolize_frame(&self, _: u64) -> Option<FrameInfo> {
                Some(FrameInfo {
                    function: "baz".to_string(),
                    file: String::new(),
                    line: 0,
                })
            }
        }
        let chain = SymbolizerChain::new(vec![Box::new(AlwaysNone), Box::new(AlwaysBaz)]);
        let result = chain.symbolize_frame(0x1000).unwrap();
        assert_eq!(result.function, "baz");
    }

    #[test]
    fn test_symbolizer_chain_all_none() {
        struct AlwaysNone;
        impl Symbolizer for AlwaysNone {
            fn symbolize_frame(&self, _: u64) -> Option<FrameInfo> {
                None
            }
        }
        let chain = SymbolizerChain::new(vec![Box::new(AlwaysNone)]);
        assert!(chain.symbolize_frame(0x1000).is_none());
    }
}
