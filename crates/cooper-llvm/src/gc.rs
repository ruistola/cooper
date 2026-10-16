//! Type descriptors for the garbage collector: which words of a value are pointers.
//!
//! Every heap allocation is made through `cooper_alloc(size, type)`. `type` describes one value
//! of the type allocated; an allocation of several values (an array's storage) is that
//! description tiled. A null `type` means the allocation holds no pointers, so the collector
//! never scans it: string bytes, and arrays of numbers.
//!
//! The descriptor is a constant (the C `CooperType`): the value's size as LLVM computes it, the
//! size this module's own layout computation arrived at (the runtime checks that they agree, which
//! validates that layout), flags, the number of 8-byte words it covers, one bit per word set when
//! the word holds a pointer, and a list of *variant sites*.
//!
//! A sum type's pointer words depend on which variant a value holds, so they are not in the
//! bitmap. Each sum type value with pointers in some variant is a site: where its tag is, where its
//! payload starts, and for each variant the descriptor of the payload (null when that variant holds
//! no pointers). The collector reads the tag and scans only the payload of the variant held. A
//! payload's descriptor has sites of its own for sum types nested in it.

use cooper_frontend::types::{integer_bits, is_float_name, is_integer_name, Type};

use crate::{mangle, Module};

/// A sum type value whose pointer words depend on its variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Site {
    /// The byte offset of the tag (a `u32`).
    pub tag: usize,
    /// The byte offset of the payload area.
    pub payload: usize,
    /// The descriptor of each variant's payload, by tag; `None` when it holds no pointers.
    pub variants: Vec<Option<String>>,
}

/// The size, alignment, and pointer words of a value, laid out as LLVM does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Layout {
    pub size: usize,
    pub align: usize,
    /// The byte offsets of the pointers in a value that are pointers whatever its variants, ascending.
    pub pointers: Vec<usize>,
    pub sites: Vec<Site>,
}

impl Layout {
    fn scalar(size: usize) -> Layout {
        Layout { size, align: size, ..Layout::default() }
    }

    fn pointers(count: usize) -> Layout {
        Layout { size: count * 8, align: 8, pointers: (0..count).map(|i| i * 8).collect(), sites: Vec::new() }
    }

    fn holds_pointers(&self) -> bool {
        !self.pointers.is_empty() || !self.sites.is_empty()
    }

    /// The layout of `fields` laid out one after another, as an LLVM struct is.
    fn sequence(fields: &[Layout]) -> Layout {
        let mut out = Layout { size: 0, align: 1, ..Layout::default() };
        for field in fields {
            out.size = out.size.next_multiple_of(field.align.max(1));
            out.pointers.extend(field.pointers.iter().map(|p| p + out.size));
            out.sites.extend(field.sites.iter().map(|s| Site { tag: s.tag + out.size, payload: s.payload + out.size, variants: s.variants.clone() }));
            out.size += field.size;
            out.align = out.align.max(field.align);
        }
        out.size = out.size.next_multiple_of(out.align);
        out
    }

    /// This layout followed by a scalar of `size` bytes.
    fn then_scalar(self, size: usize) -> Layout {
        Layout::sequence(&[self, Layout::scalar(size)])
    }
}

impl Module<'_> {
    /// The layout of a value of type `ty`, defining the descriptors of the sum type payloads in it.
    pub(crate) fn gc_layout(&mut self, ty: &Type) -> Layout {
        match ty {
            Type::Unit => Layout { size: 0, align: 1, ..Layout::default() },
            Type::Primitive(name) if name == "string" => Layout::pointers(1).then_scalar(8),
            Type::Primitive(name) if name == "bool" => Layout::scalar(1),
            Type::Primitive(name) if name == "rune" => Layout::scalar(4),
            Type::Primitive(name) if is_integer_name(name) => Layout::scalar(integer_bits(name) as usize / 8),
            Type::Primitive(name) if is_float_name(name) => Layout::scalar(if name == "f32" { 4 } else { 8 }),
            Type::Primitive(_) => Layout::scalar(8),
            Type::Pointer(_) | Type::Nil => Layout::pointers(1),
            // A function value is its code and its environment.
            Type::Func { .. } => Layout::pointers(2),
            // The elements, their count, and the capacity.
            Type::Array(_) => Layout::pointers(1).then_scalar(8).then_scalar(8),
            Type::StaticArray(elem, len) => {
                let elem = self.gc_layout(elem);
                let mut out = Layout { size: elem.size.saturating_mul(*len as usize), align: elem.align, ..Layout::default() };
                for index in 0..*len as usize {
                    let at = index * elem.size;
                    out.pointers.extend(elem.pointers.iter().map(|p| p + at));
                    out.sites.extend(elem.sites.iter().map(|s| Site { tag: s.tag + at, payload: s.payload + at, variants: s.variants.clone() }));
                }
                out
            }
            Type::Tuple(elems) => {
                let fields: Vec<Layout> = elems.iter().map(|e| self.gc_layout(e)).collect();
                Layout::sequence(&fields)
            }
            Type::Struct { .. } => {
                let members = self.defs.struct_members(ty).unwrap_or_default();
                let fields: Vec<Layout> = members.iter().map(|(_, m)| self.gc_layout(m)).collect();
                Layout::sequence(&fields)
            }
            Type::Oneof { .. } => {
                // `{ i32 tag, [words x i64] payload }`: the payload starts at byte 8, and holds
                // each variant's slots laid out as a struct.
                let mut words = 0;
                let mut variants = Vec::new();
                let base = format!("@cooper.type.{}", mangle::type_name(ty));
                for (index, variant) in self.defs.variant_order(ty).to_vec().iter().enumerate() {
                    let slots = self.defs.payload(ty, variant).unwrap_or_default();
                    let fields: Vec<Layout> = slots.iter().map(|s| self.gc_layout(s)).collect();
                    let payload = Layout::sequence(&fields);
                    words = words.max(self.size_bound(&Type::Tuple(slots.clone())).div_ceil(8));
                    if payload.holds_pointers() {
                        let name = format!("{base}.v{index}");
                        if let Ok(llvm) = self.payload_type(&slots) {
                            self.define_descriptor(&name, &llvm, &payload);
                            variants.push(Some(name));
                            continue;
                        }
                    }
                    variants.push(None);
                }
                let sites = if variants.iter().any(Option::is_some) {
                    vec![Site { tag: 0, payload: 8, variants }]
                } else {
                    Vec::new()
                };
                Layout { size: 8 + words * 8, align: 8, pointers: Vec::new(), sites }
            }
            Type::Unknown | Type::Infer(_) | Type::TypeParam(_) | Type::Module(_) => Layout::default(),
        }
    }

    /// The descriptor operand (`ptr @name`'s name) for allocations of values of type `ty`, or
    /// `None` when they hold no pointers.
    pub(crate) fn type_descriptor(&mut self, ty: &Type) -> Option<String> {
        let layout = self.gc_layout(ty);
        if !layout.holds_pointers() {
            return None;
        }
        let name = format!("@cooper.type.{}", mangle::type_name(ty));
        let llvm = self.llvm_type(ty).ok().flatten()?;
        self.define_descriptor(&name, &llvm, &layout);
        Some(name)
    }

    /// The descriptor of `count` pointers in a row (a closure's environment).
    pub(crate) fn pointers_descriptor(&mut self, count: usize) -> Option<String> {
        if count == 0 {
            return None;
        }
        let name = format!("@cooper.type.pointers{count}");
        self.define_descriptor(&name, &format!("[{count} x ptr]"), &Layout::pointers(count));
        Some(name)
    }

    fn define_descriptor(&mut self, name: &str, llvm: &str, layout: &Layout) {
        if self.descriptors.contains_key(name) {
            return;
        }
        let words = layout.size.div_ceil(8);
        let mut bitmap = vec![0u64; words.div_ceil(64).max(1)];
        for pointer in &layout.pointers {
            debug_assert!(pointer % 8 == 0, "a pointer is word-aligned");
            bitmap[pointer / 8 / 64] |= 1 << (pointer / 8 % 64);
        }
        let bits = bitmap.iter().map(|b| format!("i64 {}", *b as i64)).collect::<Vec<_>>().join(", ");
        let mut text = String::new();
        // Each site's table of variant descriptors, then the table of sites.
        let mut site_entries = Vec::new();
        for (index, site) in layout.sites.iter().enumerate() {
            let table = format!("{name}.s{index}");
            let entries: Vec<String> =
                site.variants.iter().map(|v| format!("ptr {}", v.as_deref().unwrap_or("null"))).collect();
            text.push_str(&format!(
                "{table} = private constant [{} x ptr] [{}]\n",
                entries.len(),
                entries.join(", ")
            ));
            site_entries.push(format!(
                "{{ i64, i64, i64, ptr }} {{ i64 {}, i64 {}, i64 {}, ptr {table} }}",
                site.tag,
                site.payload,
                site.variants.len()
            ));
        }
        let sites = if site_entries.is_empty() {
            "null".to_string()
        } else {
            let table = format!("{name}.sites");
            text.push_str(&format!(
                "{table} = private constant [{} x {{ i64, i64, i64, ptr }}] [{}]\n",
                site_entries.len(),
                site_entries.join(", ")
            ));
            table
        };
        text.push_str(&format!(
            "{name} = private constant {{ i64, i64, i64, i64, i64, ptr, [{n} x i64] }} {{ i64 ptrtoint (ptr getelementptr ({llvm}, ptr null, i32 1) to i64), i64 {size}, i64 0, i64 {words}, i64 {nsites}, ptr {sites}, [{n} x i64] [{bits}] }}\n",
            n = bitmap.len(),
            size = layout.size,
            nsites = layout.sites.len(),
        ));
        self.descriptors.insert(name.to_string(), text);
    }
}
